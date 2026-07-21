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

private data class MotionFrame(
    val index: Int,
    val time: Double,
    val dt: Double,
    val value: Double,
    val velocity: Double,
    val target: Double,
    val driver: String,
    val timelineProgress: Double?,
    val blendingWeight: Double?,
    val running: Boolean,
)

private data class MotionScenario(
    val name: String,
    val frames: List<MotionFrame>,
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

private fun Double.jsonNumber(): String = when {
    isNaN() -> error("NaN is not valid ground truth")
    isInfinite() -> error("Infinity is not valid ground truth")
    else -> toString()
}

private fun kotlinFrameSeconds(delta: Float): Double {
    val nanos = (delta.toDouble() * NANOS_PER_SECOND).toLong()
    return (nanos.toDouble() / NANOS_PER_SECOND).toFloat().toDouble()
}

private data class ScalarSample(val value: Double, val velocity: Double, val completed: Boolean)

private fun timelineSample(from: Double, to: Double, elapsed: Double, duration: Double): ScalarSample {
    val progress = if (duration <= 0.0) 1.0 else (elapsed / duration).coerceIn(0.0, 1.0)
    val completed = progress >= 1.0
    return ScalarSample(
        value = from + (to - from) * progress,
        velocity = if (completed || duration <= 0.0) 0.0 else (to - from) / duration,
        completed = completed,
    )
}

private fun easeInOut(value: Double): Pair<Double, Double> = if (value < 0.5) {
    2.0 * value * value to 4.0 * value
} else {
    (-1.0 + (4.0 - 2.0 * value) * value) to (4.0 - 4.0 * value)
}

private fun tweenComposite(
    outgoingValue: Double,
    outgoingVelocity: Double,
    incoming: ScalarSample,
    blendElapsed: Double,
    blendDuration: Double,
): Triple<ScalarSample, Double, Double> {
    val raw = if (blendDuration <= 0.0) 1.0 else (blendElapsed / blendDuration).coerceIn(0.0, 1.0)
    val (weight, derivative) = easeInOut(raw)
    val weightVelocity = if (blendDuration > 0.0 && raw < 1.0) {
        derivative / blendDuration
    } else {
        0.0
    }
    val completed = raw >= 1.0 && incoming.completed
    val value = outgoingValue + (incoming.value - outgoingValue) * weight
    val velocity = if (completed) {
        0.0
    } else {
        (1.0 - weight) * outgoingVelocity +
            weight * incoming.velocity +
            weightVelocity * (incoming.value - outgoingValue)
    }
    return Triple(ScalarSample(value, velocity, completed), raw, weight)
}

private fun holdToTimelineTween(): MotionScenario {
    val delta = 1f / 60f
    var elapsed = 0.0
    var timelineElapsed = 0.0
    var blendElapsed = 0.0
    val frames = mutableListOf<MotionFrame>()
    for (index in 0..60) {
        val incoming = timelineSample(1.0, 2.0, timelineElapsed, 0.3)
        val (sample, rawBlend, weight) = tweenComposite(1.0, 0.0, incoming, blendElapsed, 0.1)
        frames += MotionFrame(
            index,
            elapsed,
            if (index == 0) 0.0 else kotlinFrameSeconds(delta),
            sample.value,
            sample.velocity,
            2.0,
            "timeline+tween",
            (timelineElapsed / 0.3).coerceIn(0.0, 1.0),
            weight,
            !sample.completed,
        )
        if (sample.completed) break
        val dt = kotlinFrameSeconds(delta)
        elapsed += dt
        timelineElapsed = (timelineElapsed + dt).coerceAtMost(0.3)
        blendElapsed = (blendElapsed + dt).coerceAtMost(0.1)
        check(rawBlend <= 1.0)
    }
    return MotionScenario("stopped_hold_to_timeline_tween", frames)
}

private fun springToTimelineTween(): MotionScenario {
    var presented = 0f
    var clockNanos = INITIAL_CLOCK_NANOS
    val delta = 1f / 60f
    val frameDriver = ManualOMotionFrameDriver()
    val controller = OMotionValueController.bindWithFrameDriver(
        converter = OMotionVectorConverters.Float,
        readCurrent = { presented },
        applyValue = { presented = it },
        frameDriver = frameDriver,
    )
    controller.animateTo(1f, OMotionSpringSpec(duration = 0.45f, bounce = 0.1f))
    frameDriver.advance(clockNanos)
    repeat(6) {
        clockNanos += (delta.toDouble() * NANOS_PER_SECOND).toLong()
        frameDriver.advance(clockNanos)
    }

    var elapsed = 0.0
    var timelineElapsed = 0.0
    var blendElapsed = 0.0
    val frames = mutableListOf<MotionFrame>()
    for (index in 0..60) {
        val incoming = timelineSample(1.0, 2.0, timelineElapsed, 0.3)
        val (sample, _, weight) = tweenComposite(
            presented.toDouble(),
            (controller.velocity ?: 0f).toDouble(),
            incoming,
            blendElapsed,
            0.12,
        )
        frames += MotionFrame(
            index,
            elapsed,
            if (index == 0) 0.0 else kotlinFrameSeconds(delta),
            sample.value,
            sample.velocity,
            2.0,
            "timeline+tween",
            (timelineElapsed / 0.3).coerceIn(0.0, 1.0),
            weight,
            !sample.completed,
        )
        if (sample.completed) break
        val dt = kotlinFrameSeconds(delta)
        elapsed += dt
        timelineElapsed = (timelineElapsed + dt).coerceAtMost(0.3)
        blendElapsed = (blendElapsed + dt).coerceAtMost(0.12)
        clockNanos += (delta.toDouble() * NANOS_PER_SECOND).toLong()
        frameDriver.advance(clockNanos)
    }
    return MotionScenario("running_spring_to_timeline_tween", frames)
}

private fun writeMotionFrame(writer: java.io.Writer, frame: MotionFrame) {
    writer.append("      {\"frame\": ").append(frame.index.toString())
        .append(", \"time\": ").append(frame.time.jsonNumber())
        .append(", \"dt\": ").append(frame.dt.jsonNumber())
        .append(", \"value\": ").append(frame.value.jsonNumber())
        .append(", \"velocity\": ").append(frame.velocity.jsonNumber())
        .append(", \"target\": ").append(frame.target.jsonNumber())
        .append(", \"driver\": \"").append(frame.driver).append("\"")
        .append(", \"timelineProgress\": ")
        .append(frame.timelineProgress?.jsonNumber() ?: "null")
        .append(", \"blendingWeight\": ")
        .append(frame.blendingWeight?.jsonNumber() ?: "null")
        .append(", \"running\": ").append(frame.running.toString())
        .append(", \"completed\": ").append((!frame.running).toString()).append("}")
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
                    .append(", \"driver\": \"spring\", \"timelineProgress\": null, \"blendingWeight\": null")
                    .append(", \"running\": ").append(frame.running.toString())
                    .append(", \"completed\": ").append((!frame.running).toString()).append("}")
                if (frameIndex != frames.lastIndex) writer.append(',')
                writer.append('\n')
            }
            writer.append("    ]}")
            if (scenarioIndex != scenarios.lastIndex) writer.append(',')
            writer.append('\n')
        }
        writer.append("  ],\n")
        writer.append("  \"motionScenarios\": [\n")
        val motionScenarios = listOf(holdToTimelineTween(), springToTimelineTween())
        motionScenarios.forEachIndexed { scenarioIndex, scenario ->
            writer.append("    {\"name\": \"").append(scenario.name).append("\", \"frames\": [\n")
            scenario.frames.forEachIndexed { frameIndex, frame ->
                writeMotionFrame(writer, frame)
                if (frameIndex != scenario.frames.lastIndex) writer.append(',')
                writer.append('\n')
            }
            writer.append("    ]}")
            if (scenarioIndex != motionScenarios.lastIndex) writer.append(',')
            writer.append('\n')
        }
        writer.append("  ]\n}\n")
    }
}

fun main(args: Array<String>) {
    require(args.size == 1) { "usage: OMotionGroundTruthExporter <output.json>" }
    export(File(args[0]))
}
