package com.nodeforge.player

import android.app.Activity
import android.content.pm.ActivityInfo
import android.graphics.Color
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.view.Choreographer
import android.view.Gravity
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.WindowInsets
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.TextView
import java.util.concurrent.atomic.AtomicBoolean

class MainActivity : Activity(), SurfaceHolder.Callback, Choreographer.FrameCallback {
    companion object { init { System.loadLibrary("node_forge_android_player") } }
    private external fun nativeInit(surface: Surface, width: Int, height: Int): String
    private external fun nativeFrame(delta: Double): String
    private external fun nativeResize(width: Int, height: Int)
    private external fun nativeDestroy()
    private external fun nativeInput(input: String)
    private val renderThread = HandlerThread("NodeForgeRenderer")
    private lateinit var renderer: Handler
    private lateinit var view: SurfaceView
    private lateinit var status: TextView
    private val pending = AtomicBoolean(false)
    @Volatile private var surfaceReady = false
    private var active = false
    private var lastFrame = 0L
    private var lastStatus = ""
    private var lastDisplayReport = 0L

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.colorMode = ActivityInfo.COLOR_MODE_HDR
        window.setDesiredHdrHeadroom(4f)
        window.attributes = window.attributes.apply { layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS }
        window.setDecorFitsSystemWindows(false)
        renderThread.start(); renderer = Handler(renderThread.looper)
        view = SurfaceView(this)
        view.setDesiredHdrHeadroom(4f)
        view.holder.addCallback(this)
        view.setOnTouchListener { _, event ->
            val type = when (event.actionMasked) {
                android.view.MotionEvent.ACTION_DOWN -> "mousedown"
                android.view.MotionEvent.ACTION_UP, android.view.MotionEvent.ACTION_CANCEL -> "mouseup"
                else -> "mousemove"
            }
            val input = org.json.JSONObject().put("type", type).put("x", event.x).put("y", event.y).toString()
            renderer.post { if (surfaceReady) nativeInput(input) }
            true
        }
        status = TextView(this).apply {
            setTextColor(Color.WHITE); textSize = 14f; setPadding(24, 24, 24, 24)
            setBackgroundColor(0x99000000.toInt()); text = "Node Forge · Preparing HDR renderer"
        }
        val root = FrameLayout(this)
        root.addView(view, FrameLayout.LayoutParams(-1, -1))
        root.addView(status, FrameLayout.LayoutParams(-2, -2, Gravity.BOTTOM or Gravity.CENTER_HORIZONTAL))
        setContentView(root)
        window.insetsController?.hide(WindowInsets.Type.systemBars())
    }

    private fun showStatus(message: String) {
        if (message == lastStatus) return
        lastStatus = message
        runOnUiThread { status.text = message; status.visibility = if (message.isEmpty()) android.view.View.GONE else android.view.View.VISIBLE }
    }

    override fun surfaceCreated(holder: SurfaceHolder) {}
    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        renderer.post {
            if (surfaceReady) nativeResize(width, height)
            else {
                val error = nativeInit(holder.surface, width, height)
                surfaceReady = error.isEmpty()
                showStatus(if (surfaceReady) "Node Forge · HDR ready" else error)
            }
        }
    }
    override fun surfaceDestroyed(holder: SurfaceHolder) {
        surfaceReady = false
        val done = java.util.concurrent.CountDownLatch(1)
        renderer.post { nativeDestroy(); done.countDown() }
        done.await()
    }
    override fun onResume() { super.onResume(); active = true; lastFrame = 0L; Choreographer.getInstance().postFrameCallback(this) }
    override fun onPause() { active = false; Choreographer.getInstance().removeFrameCallback(this); lastFrame = 0L; super.onPause() }
    override fun doFrame(time: Long) {
        if (!active) return
        if (surfaceReady && time - lastDisplayReport > 1_000_000_000L) {
            lastDisplayReport = time
            val metrics = org.json.JSONObject().put("type", "display_metrics")
                .put("refreshRate", display?.refreshRate ?: 0f)
                .put("hdrSdrRatio", if (display?.isHdrSdrRatioAvailable == true) display?.hdrSdrRatio else org.json.JSONObject.NULL)
                .put("desiredHdrHeadroom", 4)
                .toString()
            renderer.post { if (surfaceReady) nativeInput(metrics) }
        }
        if (surfaceReady && pending.compareAndSet(false, true)) {
            val dt = if (lastFrame == 0L) 0.0 else (time-lastFrame)/1_000_000_000.0
            lastFrame = time
            renderer.post {
                try { if (surfaceReady) { val message = nativeFrame(dt); showStatus(message) } }
                finally { pending.set(false) }
            }
        }
        Choreographer.getInstance().postFrameCallback(this)
    }
    override fun onDestroy() { renderer.post { nativeDestroy() }; renderThread.quitSafely(); super.onDestroy() }
}
