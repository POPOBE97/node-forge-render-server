package com.nodeforge.screentest

import android.app.Activity
import android.content.pm.ActivityInfo
import android.opengl.GLES30 as GL
import android.opengl.GLSurfaceView
import android.os.Bundle
import android.view.WindowInsets
import android.view.WindowManager
import android.view.Surface
import org.json.JSONObject
import org.json.JSONArray
import java.net.ServerSocket
import java.net.InetAddress
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import javax.microedition.khronos.egl.*
import javax.microedition.khronos.opengles.GL10

class MainActivity : Activity(), GLSurfaceView.Renderer {
    private lateinit var view: GLSurfaceView
    @Volatile private var mode = "Srgb"
    @Volatile private var generation = ""
    @Volatile private var pendingGeneration = ""
    @Volatile private var active = false
    @Volatile private var error: String? = null
    @Volatile private var extensions = ""
    @Volatile private var dimensions = intArrayOf(0,0)
    private var program = 0
    private var texture = 0
    private var server: ServerSocket? = null
    private var chart: ByteArray? = null
    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        window.colorMode=ActivityInfo.COLOR_MODE_WIDE_COLOR_GAMUT
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.setDecorFitsSystemWindows(false)
        window.attributes = window.attributes.apply { layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS }
        view = GLSurfaceView(this)
        view.setEGLContextClientVersion(3)
        view.setEGLConfigChooser(GLSurfaceView.EGLConfigChooser { egl, display ->
            extensions = egl.eglQueryString(display,EGL10.EGL_EXTENSIONS) ?: ""
            val configs = arrayOfNulls<EGLConfig>(1); val count = IntArray(1)
            val attributes = intArrayOf(EGL10.EGL_RENDERABLE_TYPE,0x40,EGL10.EGL_RED_SIZE,16,EGL10.EGL_GREEN_SIZE,16,EGL10.EGL_BLUE_SIZE,16,EGL10.EGL_ALPHA_SIZE,16,0x3339,0x333B,EGL10.EGL_NONE)
            if (!egl.eglChooseConfig(display,attributes,configs,1,count) || count[0]==0) {
                error="FP16 EGL configuration unavailable; display measurement disabled"
                val fallback=intArrayOf(EGL10.EGL_RENDERABLE_TYPE,0x40,EGL10.EGL_RED_SIZE,8,EGL10.EGL_GREEN_SIZE,8,EGL10.EGL_BLUE_SIZE,8,EGL10.EGL_ALPHA_SIZE,8,EGL10.EGL_NONE)
                check(egl.eglChooseConfig(display,fallback,configs,1,count)&&count[0]>0){"No usable display surface"}
            }
            configs[0]
        })
        view.setEGLWindowSurfaceFactory(object: GLSurfaceView.EGLWindowSurfaceFactory {
            override fun createWindowSurface(egl:EGL10,display:EGLDisplay,config:EGLConfig,nativeWindow:Any):EGLSurface {
                val color = if(mode=="DisplayP3") 0x3362 else 0x3350
                val surface=egl.eglCreateWindowSurface(display,config,nativeWindow,if(error==null) intArrayOf(0x309D,color,EGL10.EGL_NONE) else intArrayOf(EGL10.EGL_NONE))
                if(surface==EGL10.EGL_NO_SURFACE) error="Cannot create the requested linear color-space surface: EGL ${egl.eglGetError()}"
                return surface
            }
            override fun destroySurface(egl:EGL10,display:EGLDisplay,surface:EGLSurface) { egl.eglDestroySurface(display,surface) }
        })
        view.setRenderer(this); view.renderMode=GLSurfaceView.RENDERMODE_WHEN_DIRTY
        setContentView(view)
        // The window's decor does not exist before setContentView on some devices.
        // Insets control is available only after the view is attached to the window.
        view.post { hideSystemBars() }
        Thread({ serve() },"screen-test-usb").apply { isDaemon=true }.start()
    }
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) hideSystemBars()
    }
    private fun hideSystemBars() {
        view.windowInsetsController?.hide(WindowInsets.Type.systemBars())
    }
    override fun onResume() { super.onResume(); active=true; if(::view.isInitialized) view.onResume() }
    override fun onPause() { active=false; generation=""; if(::view.isInitialized) view.onPause(); super.onPause() }
    override fun onDestroy() { server?.close(); super.onDestroy() }
    private fun status():JSONObject {
        val display=display!!; val dm=display.mode
        val insets=window.decorView.rootWindowInsets?.displayCutout
        val corner=window.decorView.rootWindowInsets?.getRoundedCorner(android.view.RoundedCorner.POSITION_TOP_LEFT)?.radius ?: 0
        val modes=JSONArray(); if(extensions.contains("EGL_EXT_gl_colorspace_scrgb_linear")) modes.put("Srgb")
        if(extensions.contains("EGL_EXT_gl_colorspace_display_p3_linear")&&display.isWideColorGamut) modes.put("DisplayP3")
        if(display.isHdr&&extensions.contains("EGL_EXT_gl_colorspace_scrgb_linear")) modes.put("LinearHdr")
        val d=JSONObject().put("id","usb").put("platform","Android").put("name",android.os.Build.MODEL)
            .put("size",JSONArray(listOf(dm.physicalWidth,dm.physicalHeight))).put("drawable",JSONArray(dimensions.toList()))
            .put("insets",JSONArray(listOf(maxOf(insets?.safeInsetLeft?:0,corner/2),maxOf(insets?.safeInsetBottom?:0,corner/2),maxOf(insets?.safeInsetRight?:0,corner/2),maxOf(insets?.safeInsetTop?:0,corner/2))))
            .put("modes",modes).put("brightness",window.attributes.screenBrightness.toDouble())
            .put("headroom",if(display.isHdrSdrRatioAvailable) display.hdrSdrRatio.toDouble() else 1.0)
            .put("active",active).put("generation",generation)
        error?.let { d.put("error",it) }; return d
    }
    private fun present(header:JSONObject,payload:ByteArray) {
        check(active) { "Keep player foreground" }
        val manifest=header.getJSONObject("manifest"); val size=manifest.getJSONArray("size")
        val width=size.getInt(0);val height=size.getInt(1)
        check(width>0&&height>0&&width.toLong()*height*8==payload.size.toLong()) { "Invalid image size" }
        val dm=display!!.mode;check(width==dm.physicalWidth&&height==dm.physicalHeight) { "Native screen size mismatch" }
        val hash=MessageDigest.getInstance("SHA-256").digest(payload).joinToString("") { "%02x".format(it) }
        check(hash==manifest.getString("sha256")) { "Chart checksum mismatch" }
        val requested=manifest.getString("mode"); check((0 until status().getJSONArray("modes").length()).any {status().getJSONArray("modes").getString(it)==requested}) { "Requested color mode unavailable" }
        val brightness=header.getDouble("brightness");check(brightness in 0.0..1.0)
        generation="";pendingGeneration=manifest.getString("generation");chart=payload;error=null
        window.attributes=window.attributes.apply { screenBrightness=brightness.toFloat() }
        window.colorMode=if(requested=="LinearHdr") ActivityInfo.COLOR_MODE_HDR else ActivityInfo.COLOR_MODE_WIDE_COLOR_GAMUT
        window.setDesiredHdrHeadroom(if(requested=="LinearHdr") manifest.getDouble("peak").toFloat() else 1f)
        view.setDesiredHdrHeadroom(if(requested=="LinearHdr") manifest.getDouble("peak").toFloat() else 1f)
        if(requested!=mode) {view.onPause();mode=requested;view.onResume()}
        else {view.queueEvent{ upload() };view.requestRender()}
    }
    override fun onSurfaceCreated(gl:GL10?,config:EGLConfig?) {
        try {
            val vertex="""#version 300 es
                out vec2 uv;
                void main(){vec2 p=vec2((gl_VertexID==1)?3.0:-1.0,(gl_VertexID==2)?3.0:-1.0);gl_Position=vec4(p,0,1);uv=vec2((p.x+1.0)/2.0,1.0-(p.y+1.0)/2.0);}
            """
            val fragment="""#version 300 es
                precision highp float;in vec2 uv;uniform sampler2D chart;out vec4 color;
                void main(){color=texture(chart,uv);}
            """
            fun shader(type:Int,source:String):Int {val s=GL.glCreateShader(type);GL.glShaderSource(s,source);GL.glCompileShader(s);val ok=IntArray(1);GL.glGetShaderiv(s,GL.GL_COMPILE_STATUS,ok,0);check(ok[0]!=0){GL.glGetShaderInfoLog(s)};return s}
            program=GL.glCreateProgram();GL.glAttachShader(program,shader(GL.GL_VERTEX_SHADER,vertex));GL.glAttachShader(program,shader(GL.GL_FRAGMENT_SHADER,fragment));GL.glLinkProgram(program)
            val ok=IntArray(1);GL.glGetProgramiv(program,GL.GL_LINK_STATUS,ok,0);check(ok[0]!=0){GL.glGetProgramInfoLog(program)}
            val textures=IntArray(1);GL.glGenTextures(1,textures,0);texture=textures[0];upload()
        } catch(e:Exception) {error=e.toString()}
    }
    override fun onSurfaceChanged(gl:GL10?,width:Int,height:Int) {dimensions=intArrayOf(width,height);GL.glViewport(0,0,width,height);upload()}
    private fun upload() {
        val bytes=chart?:return;val dm=display!!.mode
        GL.glBindTexture(GL.GL_TEXTURE_2D,texture);GL.glTexParameteri(GL.GL_TEXTURE_2D,GL.GL_TEXTURE_MIN_FILTER,GL.GL_NEAREST);GL.glTexParameteri(GL.GL_TEXTURE_2D,GL.GL_TEXTURE_MAG_FILTER,GL.GL_NEAREST)
        GL.glTexParameteri(GL.GL_TEXTURE_2D,GL.GL_TEXTURE_WRAP_S,GL.GL_CLAMP_TO_EDGE);GL.glTexParameteri(GL.GL_TEXTURE_2D,GL.GL_TEXTURE_WRAP_T,GL.GL_CLAMP_TO_EDGE)
        val buffer=ByteBuffer.allocateDirect(bytes.size).order(ByteOrder.LITTLE_ENDIAN);buffer.put(bytes).position(0)
        GL.glTexImage2D(GL.GL_TEXTURE_2D,0,GL.GL_RGBA16F,dm.physicalWidth,dm.physicalHeight,0,GL.GL_RGBA,GL.GL_HALF_FLOAT,buffer)
        val code=GL.glGetError();if(code!=GL.GL_NO_ERROR)error="Texture upload GL error $code"
        view.requestRender()
    }
    override fun onDrawFrame(gl:GL10?) {
        GL.glClearColor(0.05f,0.05f,0.05f,1f);GL.glClear(GL.GL_COLOR_BUFFER_BIT)
        if(texture!=0&&chart!=null&&error==null){GL.glUseProgram(program);GL.glBindTexture(GL.GL_TEXTURE_2D,texture);GL.glDrawArrays(GL.GL_TRIANGLES,0,3);GL.glFinish()
            // A second vsync is requested before confirmation, after GLSurfaceView's buffer swap.
            val expected=pendingGeneration
            view.postOnAnimation{view.postOnAnimation{if(expected==pendingGeneration&&active)generation=expected}}
        }
    }
    private fun serve() {
        try {
            server=ServerSocket(48731,4,InetAddress.getByName("127.0.0.1"))
            while(!isFinishing){val socket=server!!.accept();socket.use{socket.soTimeout=15000
                try {
                    val input=socket.getInputStream();val prefix=input.readNBytes(4);check(prefix.size==4)
                    val length=ByteBuffer.wrap(prefix).order(ByteOrder.LITTLE_ENDIAN).int;check(length in 1..1048576)
                    val header=JSONObject(String(input.readNBytes(length),Charsets.UTF_8));val n=header.optInt("payload_bytes");check(n in 0..160000000)
                    val bytes=input.readNBytes(n);check(bytes.size==n)
                    var result=JSONObject();val done=CountDownLatch(1)
                    runOnUiThread {try {when(header.getString("command")){"present"->present(header,bytes);"status"->{};else->error("Unknown command")};result=status()}catch(e:Exception){result=JSONObject().put("error",e.toString())}finally{done.countDown()}}
                    check(done.await(12,TimeUnit.SECONDS)){"UI timed out"}
                    val out=result.toString().toByteArray();socket.getOutputStream().write(ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(out.size).array());socket.getOutputStream().write(out)
                }catch(e:Exception){runCatching {val out=JSONObject().put("error",e.toString()).toString().toByteArray();socket.getOutputStream().write(ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(out.size).array());socket.getOutputStream().write(out)}}
            }}
        }catch(e:Exception){error=e.toString()}
    }
}
