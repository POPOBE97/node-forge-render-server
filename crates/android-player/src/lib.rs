mod gpu;
mod native_output;
mod player;
mod presentation;
use jni::{
    JNIEnv,
    objects::{JObject, JString},
    sys::{jdouble, jint, jstring},
};
use std::{cell::RefCell, ptr::NonNull};

thread_local! { static GPU: RefCell<Option<player::Player>> = const { RefCell::new(None) }; }

fn response(env: &JNIEnv<'_>, result: anyhow::Result<()>) -> jstring {
    let text = match result {
        Ok(()) => String::new(),
        Err(error) => {
            log::error!("{error:#}");
            format!("{error:#}")
        }
    };
    env.new_string(text)
        .map(JString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_nodeforge_player_MainActivity_nativeInit(
    env: JNIEnv,
    _this: JObject,
    surface: JObject,
    width: jint,
    height: jint,
) -> jstring {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("NodeForge")
            .with_max_level(log::LevelFilter::Info),
    );
    std::panic::set_hook(Box::new(|info| log::error!("Rust panic: {info}")));
    let result = (|| {
        GPU.with(|gpu| gpu.borrow_mut().take());
        let window = unsafe {
            ndk_sys::ANativeWindow_fromSurface(env.get_native_interface(), surface.as_raw())
        };
        let window = NonNull::new(window)
            .ok_or_else(|| anyhow::anyhow!("Android Surface is unavailable"))?;
        let gpu = unsafe { gpu::Gpu::new(window, width as u32, height as u32) }?;
        let player = player::Player::new(gpu)?;
        GPU.with(|value| *value.borrow_mut() = Some(player));
        Ok(())
    })();
    response(&env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_nodeforge_player_MainActivity_nativeFrame(
    env: JNIEnv,
    _this: JObject,
    _delta: jdouble,
) -> jstring {
    let result = GPU.with(|value| {
        if let Some(player) = value.borrow_mut().as_mut() {
            player.frame(_delta)
        } else {
            Ok("Node Forge · Surface unavailable".to_owned())
        }
    });
    let text = result.unwrap_or_else(|error| {
        log::error!("{error:#}");
        format!("{error:#}")
    });
    env.new_string(text)
        .map(JString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_nodeforge_player_MainActivity_nativeColorMode(
    _env: JNIEnv,
    _this: JObject,
) -> jint {
    GPU.with(|value| value.borrow().as_ref().map_or(0, |player| player.gpu.output().android_color_mode()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_nodeforge_player_MainActivity_nativeResize(
    env: JNIEnv,
    _this: JObject,
    width: jint,
    height: jint,
) -> jstring {
    let result = GPU.with(|value| {
        if let Some(gpu) = value.borrow_mut().as_mut() {
            gpu.gpu.resize(width as u32, height as u32)?;
        }
        Ok(())
    });
    response(&env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_nodeforge_player_MainActivity_nativeDestroy(
    _env: JNIEnv,
    _this: JObject,
) {
    GPU.with(|value| {
        value.borrow_mut().take();
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_nodeforge_player_MainActivity_nativeInput(
    mut env: JNIEnv,
    _this: JObject,
    input: JString,
) {
    let result: anyhow::Result<()> = (|| {
        let text: String = env.get_string(&input)?.into();
        let value = serde_json::from_str(&text)?;
        GPU.with(|player| {
            if let Some(player) = player.borrow_mut().as_mut() {
                player.input(value)?;
            }
            Ok(())
        })
    })();
    if let Err(error) = result {
        log::error!("Input: {error:#}");
    }
}
