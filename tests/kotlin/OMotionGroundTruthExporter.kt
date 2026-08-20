@file:OptIn(com.obric.design.motion.ExperimentalOMotionApi::class)

import com.obric.design.motion.ManualOMotionFrameDriver
import com.obric.design.motion.OMotionSpringSpec
import com.obric.design.motion.OMotionValueController
import com.obric.design.motion.OMotionVectorConverters
import java.io.File

private const val NANOS_PER_SECOND = 1_000_000_000.0
private const val INITIAL_CLOCK_NANOS = 1_000_000_000L

private data class Scenario(
    val name: String,
    val duration: Float,
    val bounce: Float,
    val deltas: List<Float>,
    val initialVelocity: Float? = null,
    val retargetFrame: Int? = null,
    val retargetValue: Float = 2f,
)

private data class Frame(
    val index: Int,
    val time: Float,
    val dt: Float,
    val value: Float,
    val velocity: Float,
    val target: Float,
    val running: Boolean,
)

private fun repeated(delta: Float, count: Int) = List(count) { delta }

private fun runScenario(scenario: Scenario): List<Frame> {
    var presented = 0f
    var target = 1f
    var clockNanos = INITIAL_CLOCK_NANOS
    var elapsed = 0f
    val frameDriver = ManualOMotionFrameDriver()
    val controller = OMotionValueController.bindWithFrameDriver(
        converter = OMotionVectorConverters.Float,
        readCurrent = { presented },
        applyValue = { presented = it },
        frameDriver = frameDriver,
    )
    val spec = OMotionSpringSpec(duration = scenario.duration, bounce = scenario.bounce)
    controller.animateTo(target, spec, scenario.initialVelocity)
    frameDriver.advance(clockNanos)

    val frames = mutableListOf(
        Frame(0, 0f, 0f, presented, controller.velocity ?: 0f, target, controller.isRunning)
    )
    for ((offset, delta) in scenario.deltas.withIndex()) {
        val frameIndex = offset + 1
        if (scenario.retargetFrame == frameIndex) {
            target = scenario.retargetValue
            check(controller.retargetTo(target, spec).appliedProperties > 0)
        }
        clockNanos += (delta.toDouble() * NANOS_PER_SECOND).toLong()
        elapsed += ((clockNanos - INITIAL_CLOCK_NANOS).toDouble() / NANOS_PER_SECOND).toFloat() - elapsed
        frameDriver.advance(clockNanos)
        frames += Frame(
            frameIndex,
            elapsed,
            delta,
            presented,
            controller.velocity ?: 0f,
            target,
            controller.isRunning,
        )
        if (!controller.isRunning) break
    }
    check(!controller.isRunning) { "${scenario.name} did not finish inside its frozen delta schedule" }
    return frames
}

private fun Float.jsonNumber(): String = when {
    isNaN() -> error("NaN is not valid ground truth")
    isInfinite() -> error("Infinity is not valid ground truth")
    else -> toString()
}

private fun export(output: File) {
    val scenarios = listOf(
        Scenario("underdamped_60hz", 0.45f, 0.25f, repeated(1f / 60f, 240)),
        Scenario("critical_60hz", 0.45f, 0f, repeated(1f / 60f, 240)),
        Scenario("overdamped_60hz", 0.45f, -0.5f, repeated(1f / 60f, 360)),
        Scenario("underdamped_30hz", 0.45f, 0.25f, repeated(1f / 30f, 180)),
        Scenario("underdamped_120hz", 0.45f, 0.25f, repeated(1f / 120f, 480)),
        Scenario(
            "non_uniform_dt",
            0.45f,
            0.1f,
            List(80) { listOf(1f / 120f, 1f / 40f, 1f / 75f, 0.041f)[it % 4] },
        ),
        Scenario("single_large_dt", 0.45f, 0.1f, listOf(0.2f) + repeated(1f / 60f, 240)),
        Scenario(
            "retarget_velocity_inheritance",
            0.5f,
            0.25f,
            repeated(1f / 60f, 300),
            initialVelocity = 4f,
            retargetFrame = 10,
        ),
        Scenario("listening_any_60hz", 0.25f, 0.15f, repeated(1f / 60f, 240)),
        Scenario("listening_blur_60hz", 0.55f, 0.20f, repeated(1f / 60f, 300)),
        Scenario("listening_position_60hz", 0.30f, 0.25f, repeated(1f / 60f, 240)),
        Scenario("listening_size_60hz", 0.40f, 0.20f, repeated(1f / 60f, 240)),
        Scenario("listening_ui_opacity_60hz", 0.30f, 0.10f, repeated(1f / 60f, 240)),
    )

    output.parentFile.mkdirs()
    output.bufferedWriter().use { writer ->
        writer.append("{\n")
        writer.append("  \"source\": {\"voiceInteractionCommit\": \"b3e4abb\", \"omotionVersion\": \"0.1.0-alpha02-SNAPSHOT\"},\n")
        writer.append("  \"scenarios\": [\n")
        scenarios.forEachIndexed { scenarioIndex, scenario ->
            val frames = runScenario(scenario)
            writer.append("    {\"name\": \"").append(scenario.name)
                .append("\", \"duration\": ").append(scenario.duration.jsonNumber())
                .append(", \"bounce\": ").append(scenario.bounce.jsonNumber())
                .append(", \"frames\": [\n")
            frames.forEachIndexed { frameIndex, frame ->
                writer.append("      {\"frame\": ").append(frame.index.toString())
                    .append(", \"time\": ").append(frame.time.jsonNumber())
                    .append(", \"dt\": ").append(frame.dt.jsonNumber())
                    .append(", \"value\": ").append(frame.value.jsonNumber())
                    .append(", \"velocity\": ").append(frame.velocity.jsonNumber())
                    .append(", \"target\": ").append(frame.target.jsonNumber())
                    .append(", \"driver\": \"spring\"")
                    .append(", \"running\": ").append(frame.running.toString())
                    .append(", \"completed\": ").append((!frame.running).toString()).append("}")
                if (frameIndex != frames.lastIndex) writer.append(',')
                writer.append('\n')
            }
            writer.append("    ]}")
            if (scenarioIndex != scenarios.lastIndex) writer.append(',')
            writer.append('\n')
        }
        writer.append("  ]\n}\n")
    }
}

fun main(args: Array<String>) {
    require(args.size == 1) { "usage: OMotionGroundTruthExporter <output.json>" }
    export(File(args[0]))
}
