//! Small, runtime-loaded binding to libgphoto2's stable C API. No CLI subprocess per frame.
//! All pointers and callbacks stay on one owning thread; owned C objects are always released
//! before unloading the library. The camera driver owns the USB/PTP protocol implementation.
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char, c_int, c_uint, c_ulong, c_void},
    fs::File,
    marker::PhantomData,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use super::model::*;

type P = *mut c_void;
type ProgressStart = unsafe extern "C" fn(P, f32, *const c_char, P) -> c_uint;
type ProgressUpdate = unsafe extern "C" fn(P, c_uint, f32, P);
type ProgressStop = unsafe extern "C" fn(P, c_uint, P);
type ErrorCallback = unsafe extern "C" fn(P, *const c_char, P);

macro_rules! api {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)+) => {
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) -> $ret,)+
            _library: libloading::Library,
        }
        impl Api {
            unsafe fn from_library(library: libloading::Library) -> Result<Self, libloading::Error> {
                Ok(Self {
                    $($name: *unsafe { library.get::<unsafe extern "C" fn($($arg),*) -> $ret>(concat!(stringify!($name), "\0").as_bytes()) }?,)+
                    _library: library,
                })
            }
        }
    }
}

api! {
    gp_context_new() -> P;
    gp_context_unref(P) -> ();
    gp_context_set_error_func(P, Option<ErrorCallback>, P) -> ();
    gp_context_set_progress_funcs(P, Option<ProgressStart>, Option<ProgressUpdate>, Option<ProgressStop>, P) -> ();
    gp_result_as_string(c_int) -> *const c_char;
    gp_list_new(*mut P) -> c_int;
    gp_list_free(P) -> c_int;
    gp_list_count(P) -> c_int;
    gp_list_get_name(P, c_int, *mut *const c_char) -> c_int;
    gp_list_get_value(P, c_int, *mut *const c_char) -> c_int;
    gp_camera_autodetect(P, P) -> c_int;
    gp_camera_new(*mut P) -> c_int;
    gp_camera_set_port_info(P, P) -> c_int;
    gp_camera_init(P, P) -> c_int;
    gp_camera_exit(P, P) -> c_int;
    gp_camera_unref(P) -> c_int;
    gp_camera_get_summary(P, P, P) -> c_int;
    gp_camera_get_config(P, *mut P, P) -> c_int;
    gp_camera_get_single_config(P, *const c_char, *mut P, P) -> c_int;
    gp_camera_set_single_config(P, *const c_char, P, P) -> c_int;
    gp_camera_capture(P, c_int, *mut FilePath, P) -> c_int;
    gp_camera_capture_preview(P, P, P) -> c_int;
    gp_camera_wait_for_event(P, c_int, *mut c_int, *mut P, P) -> c_int;
    gp_camera_folder_list_files(P, *const c_char, P, P) -> c_int;
    gp_camera_folder_list_folders(P, *const c_char, P, P) -> c_int;
    gp_camera_file_get(P, *const c_char, *const c_char, c_int, P, P) -> c_int;
    gp_widget_free(P) -> c_int;
    gp_widget_count_children(P) -> c_int;
    gp_widget_get_child(P, c_int, *mut P) -> c_int;
    gp_widget_get_type(P, *mut c_int) -> c_int;
    gp_widget_get_name(P, *mut *const c_char) -> c_int;
    gp_widget_get_label(P, *mut *const c_char) -> c_int;
    gp_widget_get_value(P, P) -> c_int;
    gp_widget_set_value(P, *const c_void) -> c_int;
    gp_widget_get_readonly(P, *mut c_int) -> c_int;
    gp_widget_count_choices(P) -> c_int;
    gp_widget_get_choice(P, c_int, *mut *const c_char) -> c_int;
    gp_widget_get_range(P, *mut f32, *mut f32, *mut f32) -> c_int;
    gp_file_new(*mut P) -> c_int;
    gp_file_new_from_fd(*mut P, c_int) -> c_int;
    gp_file_unref(P) -> c_int;
    gp_file_get_data_and_size(P, *mut *const c_char, *mut c_ulong) -> c_int;
    gp_port_info_list_new(*mut P) -> c_int;
    gp_port_info_list_free(P) -> c_int;
    gp_port_info_list_load(P) -> c_int;
    gp_port_info_list_lookup_path(P, *const c_char) -> c_int;
    gp_port_info_list_get_info(P, c_int, *mut P) -> c_int;
}

/// PTP phones are also returned by autodetect. Select our supported camera, never the first
/// USB device and never require unrelated phones to be unplugged. Regional aliases on the
/// same physical port still identify one camera.
fn select_t4i(cameras: &[(String, String)]) -> CameraResult<(String, String)> {
    let mut selected: Option<&(String, String)> = None;
    for candidate in cameras {
        if !candidate.1.starts_with("usb:")
            || !matches!(
                candidate.0.as_str(),
                "Canon EOS 650D" | "Canon Rebel T4i" | "Canon EOS Rebel T4i" | "Canon EOS Kiss X6i"
            )
        {
            continue;
        }
        if selected.is_some_and(|previous| previous.1 != candidate.1) {
            return Err(CameraError::new(
                CameraErrorKind::Invalid,
                "More than one Canon T4i / 650D is connected. Keep one connected; phones may stay connected.",
            ));
        }
        selected = Some(candidate);
    }
    selected.cloned().ok_or_else(|| {
        let detected = cameras.iter().map(|(model, port)| format!("{model} at {port}")).collect::<Vec<_>>().join(", ");
        CameraError::new(
            CameraErrorKind::NotFound,
            format!("Canon T4i / 650D is not visible to the USB camera driver. Keep it powered on and connected; allow USB to recover if it was just released. Detected devices: {}.", if detected.is_empty() { "none" } else { &detected }),
        )
    })
}

#[repr(C)]
struct FilePath {
    name: [c_char; 128],
    folder: [c_char; 1024],
}

impl FilePath {
    fn remote(&self) -> RemoteFile {
        fn text(chars: &[c_char]) -> String {
            String::from_utf8_lossy(
                &chars
                    .iter()
                    .take_while(|&&c| c != 0)
                    .map(|&c| c as u8)
                    .collect::<Vec<_>>(),
            )
            .into_owned()
        }
        RemoteFile {
            folder: text(&self.folder),
            name: text(&self.name),
        }
    }
}

struct Owned<'a> {
    ptr: P,
    free: unsafe extern "C" fn(P) -> c_int,
    _api: PhantomData<&'a Api>,
}
impl Drop for Owned<'_> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                (self.free)(self.ptr);
            }
        }
    }
}
fn owned(ptr: P, free: unsafe extern "C" fn(P) -> c_int, _api: &Api) -> Owned<'_> {
    Owned {
        ptr,
        free,
        _api: PhantomData,
    }
}
fn cstring(value: &str) -> CameraResult<CString> {
    CString::new(value)
        .map_err(|_| CameraError::new(CameraErrorKind::Invalid, "Value contains a NUL byte"))
}
unsafe fn string(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

static SESSION_OWNED: AtomicBool = AtomicBool::new(false);

pub struct GPhoto {
    api: Api,
    camera: P,
    context: P,
    last_error: Box<RefCell<String>>,
    identity: CameraInfo,
    owns_session: bool,
}

impl GPhoto {
    pub fn load() -> CameraResult<Self> {
        let mut candidates = Vec::new();
        if let Some(path) = std::env::var_os("NODE_FORGE_GPHOTO2_LIBRARY") {
            candidates.push(path);
        }
        candidates.extend(
            [
                "/opt/homebrew/lib/libgphoto2.dylib",
                "/usr/local/lib/libgphoto2.dylib",
                "libgphoto2.dylib",
                "libgphoto2.so.6",
                "libgphoto2.so",
            ]
            .into_iter()
            .map(Into::into),
        );
        let mut failures = Vec::new();
        for path in candidates {
            // Loading is explicitly confined to the configured/local libgphoto2 installation.
            let loaded =
                unsafe { libloading::Library::new(&path).and_then(|l| Api::from_library(l)) };
            match loaded {
                Ok(api) => {
                    let context = unsafe { (api.gp_context_new)() };
                    if context.is_null() {
                        return Err(CameraError::new(
                            CameraErrorKind::Other,
                            "Unable to allocate camera context",
                        ));
                    }
                    let mut last_error = Box::<RefCell<String>>::default();
                    unsafe {
                        (api.gp_context_set_error_func)(
                            context,
                            Some(on_error),
                            (&mut *last_error as *mut RefCell<String>).cast(),
                        );
                    }
                    return Ok(Self {
                        api,
                        camera: ptr::null_mut(),
                        context,
                        last_error,
                        identity: CameraInfo::default(),
                        owns_session: false,
                    });
                }
                Err(e) => failures.push(format!("{}: {e}", path.to_string_lossy())),
            }
        }
        Err(CameraError::new(
            CameraErrorKind::Unavailable,
            format!(
                "libgphoto2 is not available. On macOS install with: brew install libgphoto2 gphoto2\nOr set NODE_FORGE_GPHOTO2_LIBRARY to the library path.\n{}",
                failures.join("\n")
            ),
        ))
    }

    fn check(&self, code: c_int, operation: &str) -> CameraResult<()> {
        if code >= 0 {
            return Ok(());
        }
        let kind = match code {
            -53 | -60 => CameraErrorKind::Occupied,
            -52 | -5 => CameraErrorKind::Disconnected,
            -110 | -10 => CameraErrorKind::Busy,
            -6 => CameraErrorKind::Unsupported,
            -115 | -114 => CameraErrorKind::Storage,
            -2 => CameraErrorKind::Invalid,
            _ => CameraErrorKind::Other,
        };
        let hint = if kind == CameraErrorKind::Occupied {
            " Close EOS Utility, Image Capture and other camera clients, then reconnect. macOS ptpcamerad may still hold the USB interface."
        } else {
            ""
        };
        Err(CameraError::new(
            kind,
            format!(
                "{operation}: {} ({code}). {}{hint}",
                unsafe { string((self.api.gp_result_as_string)(code)) },
                self.last_error.borrow()
            ),
        ))
    }

    fn connected(&self) -> CameraResult<()> {
        if self.camera.is_null() {
            Err(CameraError::new(
                CameraErrorKind::Disconnected,
                "Camera is not connected",
            ))
        } else {
            Ok(())
        }
    }

    fn list(&self) -> CameraResult<Owned<'_>> {
        let mut p = ptr::null_mut();
        self.check(
            unsafe { (self.api.gp_list_new)(&mut p) },
            "Create camera list",
        )?;
        Ok(owned(p, self.api.gp_list_free, &self.api))
    }

    fn bind_port(&self, path: &str) -> CameraResult<()> {
        let mut p = ptr::null_mut();
        self.check(
            unsafe { (self.api.gp_port_info_list_new)(&mut p) },
            "Create USB port list",
        )?;
        let ports = owned(p, self.api.gp_port_info_list_free, &self.api);
        self.check(
            unsafe { (self.api.gp_port_info_list_load)(ports.ptr) },
            "Refresh USB ports",
        )?;
        let path = cstring(path)?;
        let index = unsafe { (self.api.gp_port_info_list_lookup_path)(ports.ptr, path.as_ptr()) };
        self.check(index, "Find selected Canon USB port")?;
        let mut info = ptr::null_mut();
        self.check(
            unsafe { (self.api.gp_port_info_list_get_info)(ports.ptr, index, &mut info) },
            "Read selected Canon USB port",
        )?;
        // With an explicit USB port, gp_camera_init resolves the driver at that port only.
        // Without this binding it repeats autodetection and may open an unrelated phone.
        self.check(
            unsafe { (self.api.gp_camera_set_port_info)(self.camera, info) },
            "Select Canon USB port",
        )
    }

    fn widget(&self, id: &str) -> CameraResult<Owned<'_>> {
        self.connected()?;
        let name = cstring(id)?;
        let mut p = ptr::null_mut();
        let code = unsafe {
            (self.api.gp_camera_get_single_config)(self.camera, name.as_ptr(), &mut p, self.context)
        };
        let result = owned(p, self.api.gp_widget_free, &self.api);
        self.check(code, &format!("Read {id}"))?;
        Ok(result)
    }

    fn descriptor(&self, w: P) -> CameraResult<Option<CameraParameter>> {
        let a = &self.api;
        let (mut kind, mut read_only) = (0, 0);
        let (mut name, mut label) = (ptr::null(), ptr::null());
        unsafe {
            self.check((a.gp_widget_get_type)(w, &mut kind), "Read parameter type")?;
            self.check((a.gp_widget_get_name)(w, &mut name), "Read parameter name")?;
            self.check(
                (a.gp_widget_get_label)(w, &mut label),
                "Read parameter label",
            )?;
            self.check(
                (a.gp_widget_get_readonly)(w, &mut read_only),
                "Read parameter access",
            )?;
        }
        let (value, kind) = match kind {
            2 | 5 | 6 => {
                let mut p: *const c_char = ptr::null();
                self.check(
                    unsafe { (a.gp_widget_get_value)(w, (&mut p as *mut *const c_char).cast()) },
                    "Read parameter value",
                )?;
                let s = unsafe { string(p) };
                if kind == 2 {
                    (ParameterValue::Text(s), ParameterKind::Text)
                } else {
                    let n = unsafe { (a.gp_widget_count_choices)(w) };
                    self.check(n, "Read parameter choices")?;
                    let mut choices = Vec::new();
                    for i in 0..n {
                        self.check(
                            unsafe { (a.gp_widget_get_choice)(w, i, &mut p) },
                            "Read parameter choice",
                        )?;
                        choices.push(unsafe { string(p) });
                    }
                    (ParameterValue::Choice(s), ParameterKind::Choice(choices))
                }
            }
            3 => {
                let (mut n, mut min, mut max, mut step) = (0f32, 0f32, 0f32, 0f32);
                unsafe {
                    self.check(
                        (a.gp_widget_get_value)(w, (&mut n as *mut f32).cast()),
                        "Read parameter value",
                    )?;
                    self.check(
                        (a.gp_widget_get_range)(w, &mut min, &mut max, &mut step),
                        "Read parameter range",
                    )?;
                }
                (
                    ParameterValue::Number(n),
                    ParameterKind::Range { min, max, step },
                )
            }
            4 => {
                let mut n: c_int = 0;
                self.check(
                    unsafe { (a.gp_widget_get_value)(w, (&mut n as *mut c_int).cast()) },
                    "Read toggle",
                )?;
                (ParameterValue::Toggle(n != 0), ParameterKind::Toggle)
            }
            _ => return Ok(None),
        };
        Ok(Some(CameraParameter {
            id: unsafe { string(name) },
            label: unsafe { string(label) },
            value,
            kind,
            read_only: read_only != 0,
        }))
    }

    fn walk(&self, w: P, parameters: &mut Vec<CameraParameter>, depth: usize) -> CameraResult<()> {
        if depth > 16 {
            return Err(CameraError::new(
                CameraErrorKind::Invalid,
                "Camera configuration nesting exceeds limit",
            ));
        }
        if let Some(p) = self.descriptor(w)? {
            if !parameters.iter().any(|other| other.id == p.id) {
                parameters.push(p);
            }
        }
        let n = unsafe { (self.api.gp_widget_count_children)(w) };
        self.check(n, "Read configuration children")?;
        for i in 0..n {
            let mut child = ptr::null_mut();
            self.check(
                unsafe { (self.api.gp_widget_get_child)(w, i, &mut child) },
                "Read configuration child",
            )?;
            self.walk(child, parameters, depth + 1)?;
        }
        Ok(())
    }

    fn select_card(&mut self) -> CameraResult<()> {
        let value = {
            let widget = self.widget("capturetarget")?;
            let parameter = self.descriptor(widget.ptr)?.ok_or_else(|| {
                CameraError::new(
                    CameraErrorKind::Unsupported,
                    "Capture destination is unavailable",
                )
            })?;
            match parameter.kind {
                ParameterKind::Choice(choices) => choices
                    .into_iter()
                    .find(|v| v == "Memory card" || v == "Card"),
                _ => None,
            }
            .ok_or_else(|| {
                CameraError::new(
                    CameraErrorKind::Unsupported,
                    "Camera has no memory-card capture destination",
                )
            })?
        };
        self.set_parameter("capturetarget", &ParameterValue::Choice(value))
    }
}

impl CameraBackend for GPhoto {
    fn connect(&mut self) -> CameraResult<CameraInfo> {
        if !self.camera.is_null() {
            return self.info();
        }
        if SESSION_OWNED.swap(true, Ordering::AcqRel) {
            return Err(CameraError::new(
                CameraErrorKind::Busy,
                "Another Camera Studio session owns the camera. Finish its work and disconnect it first.",
            ));
        }
        self.owns_session = true;
        let result = (|| {
            let (model, port) = {
                let list = self.list()?;
                self.last_error.borrow_mut().clear();
                let count = unsafe { (self.api.gp_camera_autodetect)(list.ptr, self.context) };
                self.check(count, "Find camera")?;
                let mut cameras = Vec::new();
                for index in 0..count {
                    let (mut name, mut value) = (ptr::null(), ptr::null());
                    unsafe {
                        self.check(
                            (self.api.gp_list_get_name)(list.ptr, index, &mut name),
                            "Read camera name",
                        )?;
                        self.check(
                            (self.api.gp_list_get_value)(list.ptr, index, &mut value),
                            "Read camera port",
                        )?;
                        cameras.push((string(name), string(value)));
                    }
                }
                select_t4i(&cameras)?
            };
            let code = unsafe { (self.api.gp_camera_new)(&mut self.camera) };
            self.check(code, "Allocate camera")?;
            self.last_error.borrow_mut().clear();
            self.bind_port(&port)?;
            self.check(
                unsafe { (self.api.gp_camera_init)(self.camera, self.context) },
                "Open camera",
            )?;
            self.identity.model = model;
            self.identity.port = port;
            let mut summary = [0 as c_char; 32768];
            self.check(
                unsafe {
                    (self.api.gp_camera_get_summary)(
                        self.camera,
                        summary.as_mut_ptr().cast(),
                        self.context,
                    )
                },
                "Read camera summary",
            )?;
            self.identity.summary = unsafe { string(summary.as_ptr()) };
            self.info()
        })();
        if result.is_err() {
            let _ = self.disconnect();
        }
        result
    }

    fn disconnect(&mut self) -> CameraResult<()> {
        let mut code = 0;
        if !self.camera.is_null() {
            unsafe {
                code = (self.api.gp_camera_exit)(self.camera, self.context);
                (self.api.gp_camera_unref)(self.camera);
            }
            self.camera = ptr::null_mut();
        }
        if self.owns_session {
            self.owns_session = false;
            SESSION_OWNED.store(false, Ordering::Release);
        }
        self.check(code, "Close camera")
    }

    fn info(&mut self) -> CameraResult<CameraInfo> {
        self.connected()?;
        self.last_error.borrow_mut().clear();
        let mut root = ptr::null_mut();
        let code = unsafe { (self.api.gp_camera_get_config)(self.camera, &mut root, self.context) };
        let root = owned(root, self.api.gp_widget_free, &self.api);
        self.check(code, "Read camera settings")?;
        let mut info = self.identity.clone();
        info.parameters.clear();
        self.walk(root.ptr, &mut info.parameters, 0)?;
        Ok(info)
    }

    fn set_parameter(&mut self, id: &str, value: &ParameterValue) -> CameraResult<()> {
        self.last_error.borrow_mut().clear();
        let w = self.widget(id)?;
        let p = self.descriptor(w.ptr)?.ok_or_else(|| {
            CameraError::new(CameraErrorKind::Unsupported, "Unsupported parameter type")
        })?;
        if p.read_only {
            return Err(CameraError::new(
                CameraErrorKind::Unsupported,
                format!("{} is read-only in the current camera mode", p.label),
            ));
        }
        validate_parameter(&p, value)?;
        let code = match value {
            ParameterValue::Choice(s) | ParameterValue::Text(s) => {
                let s = cstring(s)?;
                unsafe { (self.api.gp_widget_set_value)(w.ptr, s.as_ptr().cast()) }
            }
            ParameterValue::Toggle(v) => {
                let n = c_int::from(*v);
                unsafe { (self.api.gp_widget_set_value)(w.ptr, (&n as *const c_int).cast()) }
            }
            ParameterValue::Number(n) => unsafe {
                (self.api.gp_widget_set_value)(w.ptr, (n as *const f32).cast())
            },
        };
        self.check(code, "Prepare parameter")?;
        let id = cstring(id)?;
        self.check(
            unsafe {
                (self.api.gp_camera_set_single_config)(
                    self.camera,
                    id.as_ptr(),
                    w.ptr,
                    self.context,
                )
            },
            "Set camera parameter",
        )
    }

    fn preview(&mut self, enabled: bool) -> CameraResult<()> {
        self.set_parameter("viewfinder", &ParameterValue::Toggle(enabled))
    }

    fn preview_frame(&mut self) -> CameraResult<Vec<u8>> {
        self.connected()?;
        self.last_error.borrow_mut().clear();
        let mut p = ptr::null_mut();
        self.check(
            unsafe { (self.api.gp_file_new)(&mut p) },
            "Allocate preview",
        )?;
        let file = owned(p, self.api.gp_file_unref, &self.api);
        self.check(
            unsafe { (self.api.gp_camera_capture_preview)(self.camera, file.ptr, self.context) },
            "Get preview",
        )?;
        let (mut data, mut size) = (ptr::null(), 0 as c_ulong);
        self.check(
            unsafe { (self.api.gp_file_get_data_and_size)(file.ptr, &mut data, &mut size) },
            "Read preview",
        )?;
        if data.is_null() || size == 0 || size > 32 * 1024 * 1024 {
            return Err(CameraError::new(
                CameraErrorKind::Invalid,
                "Camera returned an invalid preview frame",
            ));
        }
        Ok(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) }.to_vec())
    }

    fn focus(&mut self, near: bool, step: u8) -> CameraResult<()> {
        if !(1..=3).contains(&step) {
            return Err(CameraError::new(
                CameraErrorKind::Invalid,
                "Focus step must be 1, 2 or 3",
            ));
        }
        self.set_parameter(
            "manualfocusdrive",
            &ParameterValue::Choice(format!("{} {step}", if near { "Near" } else { "Far" })),
        )
    }

    fn capture(&mut self) -> CameraResult<Option<RemoteFile>> {
        {
            let widget = self.widget("shutterspeed")?;
            if self.descriptor(widget.ptr)?.is_some_and(|p| {
                matches!(p.value,
                ParameterValue::Choice(ref value) if value.eq_ignore_ascii_case("bulb"))
            }) {
                return Err(CameraError::new(
                    CameraErrorKind::Unsupported,
                    "Choose a timed shutter speed. Bulb exposures require a separate start/stop controller and are not supported by Take photo.",
                ));
            }
        }
        self.select_card()?;
        self.last_error.borrow_mut().clear();
        let mut path = FilePath {
            name: [0; 128],
            folder: [0; 1024],
        };
        self.check(
            unsafe { (self.api.gp_camera_capture)(self.camera, 0, &mut path, self.context) },
            "Capture photo (not retried)",
        )?;
        let file = path.remote();
        Ok((!file.name.is_empty()).then_some(file))
    }

    fn record(&mut self, enabled: bool) -> CameraResult<()> {
        let info = self.info()?;
        if !enabled && info.recording_to_card() == Some(false) {
            // Already idle or delivering Live View to SDRAM. Do not send a recording command.
            return Ok(());
        }
        if enabled && !info.can_start_recording() {
            return Err(CameraError::new(
                CameraErrorKind::Unsupported,
                "The camera does not offer Card recording in this mode. Set the T4i's physical power switch to movie mode and insert an SD card.",
            ));
        }
        if enabled && info.recording_to_card() == Some(true) {
            return Err(CameraError::new(
                CameraErrorKind::Busy,
                "The camera is already recording to its card",
            ));
        }
        if !enabled && info.recording_to_card() != Some(true) {
            return Err(CameraError::new(
                CameraErrorKind::Unsupported,
                "The camera's recording state is unknown. Refresh its status before trying to stop recording.",
            ));
        }
        if enabled {
            self.select_card()?;
            self.preview(true)?;
        }
        self.set_parameter(
            "movierecordtarget",
            &ParameterValue::Choice(if enabled { "Card" } else { "None" }.into()),
        )
    }

    fn poll_event(&mut self, timeout_ms: i32) -> CameraResult<BackendEvent> {
        self.connected()?;
        let (mut kind, mut data) = (0, ptr::null_mut());
        self.last_error.borrow_mut().clear();
        let code = unsafe {
            (self.api.gp_camera_wait_for_event)(
                self.camera,
                timeout_ms,
                &mut kind,
                &mut data,
                self.context,
            )
        };
        let event = match kind {
            1 => BackendEvent::Timeout,
            2 | 5 if !data.is_null() => {
                BackendEvent::FileAdded(unsafe { &*data.cast::<FilePath>() }.remote())
            }
            4 => BackendEvent::CaptureComplete,
            _ => BackendEvent::Changed,
        };
        // libgphoto2 explicitly transfers event payload ownership to the caller.
        if !data.is_null() {
            unsafe {
                libc::free(data);
            }
        }
        self.check(code, "Poll camera event")?;
        Ok(event)
    }

    fn list_files(&mut self) -> CameraResult<Vec<RemoteFile>> {
        self.connected()?;
        let mut folders = vec!["/".to_string()];
        let mut visited = std::collections::HashSet::new();
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            if !visited.insert(folder.clone()) {
                continue;
            }
            if visited.len() > 4096 {
                return Err(CameraError::new(
                    CameraErrorKind::Invalid,
                    "Camera folder count exceeds limit",
                ));
            }
            let cfolder = cstring(&folder)?;
            let list = self.list()?;
            self.check(
                unsafe {
                    (self.api.gp_camera_folder_list_files)(
                        self.camera,
                        cfolder.as_ptr(),
                        list.ptr,
                        self.context,
                    )
                },
                "List card files",
            )?;
            let count = unsafe { (self.api.gp_list_count)(list.ptr) };
            self.check(count, "Read file count")?;
            for i in 0..count {
                let mut name = ptr::null();
                self.check(
                    unsafe { (self.api.gp_list_get_name)(list.ptr, i, &mut name) },
                    "Read file name",
                )?;
                let file = RemoteFile {
                    folder: folder.clone(),
                    name: unsafe { string(name) },
                };
                if file.is_media() {
                    files.push(file);
                }
            }
            let list = self.list()?;
            self.check(
                unsafe {
                    (self.api.gp_camera_folder_list_folders)(
                        self.camera,
                        cfolder.as_ptr(),
                        list.ptr,
                        self.context,
                    )
                },
                "List card folders",
            )?;
            let count = unsafe { (self.api.gp_list_count)(list.ptr) };
            self.check(count, "Read folder count")?;
            for i in 0..count {
                let mut name = ptr::null();
                self.check(
                    unsafe { (self.api.gp_list_get_name)(list.ptr, i, &mut name) },
                    "Read folder name",
                )?;
                let name = unsafe { string(name) };
                if name == "." || name == ".." || name.contains('/') {
                    continue;
                }
                folders.push(format!("{}/{name}", folder.trim_end_matches('/')));
            }
        }
        Ok(files)
    }

    fn download(
        &mut self,
        file: &RemoteFile,
        output: &File,
        progress: &mut dyn FnMut(f32),
    ) -> CameraResult<()> {
        self.connected()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let fd = unsafe { libc::dup(output.as_raw_fd()) };
            if fd < 0 {
                return Err(CameraError::new(
                    CameraErrorKind::Storage,
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            let mut p = ptr::null_mut();
            let code = unsafe { (self.api.gp_file_new_from_fd)(&mut p, fd) };
            if code < 0 {
                unsafe {
                    libc::close(fd);
                }
                self.check(code, "Create download")?;
            }
            let target = owned(p, self.api.gp_file_unref, &self.api);
            let folder = cstring(&file.folder)?;
            let name = cstring(&file.name)?;
            let mut state = Progress {
                callback: progress,
                target: 0.0,
                panicked: false,
            };
            unsafe {
                (self.api.gp_context_set_progress_funcs)(
                    self.context,
                    Some(progress_start),
                    Some(progress_update),
                    Some(progress_stop),
                    (&mut state as *mut Progress<'_>).cast(),
                );
            }
            self.last_error.borrow_mut().clear();
            let code = unsafe {
                (self.api.gp_camera_file_get)(
                    self.camera,
                    folder.as_ptr(),
                    name.as_ptr(),
                    1,
                    target.ptr,
                    self.context,
                )
            };
            unsafe {
                (self.api.gp_context_set_progress_funcs)(
                    self.context,
                    None,
                    None,
                    None,
                    ptr::null_mut(),
                );
            }
            if state.panicked {
                return Err(CameraError::new(
                    CameraErrorKind::Other,
                    "Download progress callback failed",
                ));
            }
            self.check(code, "Download camera file")
        }
        #[cfg(not(unix))]
        {
            let _ = (file, output, progress);
            Err(CameraError::new(
                CameraErrorKind::Unsupported,
                "Camera file streaming currently requires macOS or Linux",
            ))
        }
    }
}

impl Drop for GPhoto {
    fn drop(&mut self) {
        let _ = self.disconnect();
        unsafe {
            (self.api.gp_context_unref)(self.context);
        }
    }
}

unsafe extern "C" fn on_error(_: P, text: *const c_char, data: P) {
    if !data.is_null() {
        unsafe {
            *(&*data.cast::<RefCell<String>>()).borrow_mut() = string(text);
        }
    }
}
struct Progress<'a> {
    callback: &'a mut dyn FnMut(f32),
    target: f32,
    panicked: bool,
}
impl Progress<'_> {
    fn notify(&mut self, fraction: f32) {
        if self.panicked || !fraction.is_finite() {
            return;
        }
        self.panicked =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.callback)(fraction)))
                .is_err();
    }
}
unsafe extern "C" fn progress_start(_: P, target: f32, _: *const c_char, data: P) -> c_uint {
    let state = unsafe { &mut *data.cast::<Progress<'_>>() };
    state.target = target;
    state.notify(0.0);
    1
}
unsafe extern "C" fn progress_update(_: P, _: c_uint, current: f32, data: P) {
    let state = unsafe { &mut *data.cast::<Progress<'_>>() };
    if state.target > 0.0 {
        state.notify((current / state.target).clamp(0.0, 1.0));
    }
}
unsafe extern "C" fn progress_stop(_: P, _: c_uint, _: P) {}

fn validate_parameter(p: &CameraParameter, value: &ParameterValue) -> CameraResult<()> {
    let valid = match (&p.kind, value) {
        (ParameterKind::Choice(choices), ParameterValue::Choice(v)) => choices.contains(v),
        (ParameterKind::Toggle, ParameterValue::Toggle(_)) => true,
        (ParameterKind::Range { min, max, .. }, ParameterValue::Number(v)) => {
            v.is_finite() && v >= min && v <= max
        }
        (ParameterKind::Text, ParameterValue::Text(_)) => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(CameraError::new(
            CameraErrorKind::Invalid,
            format!("{} does not accept this value in the current mode", p.label),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(model: &str, port: &str) -> (String, String) {
        (model.into(), port.into())
    }

    #[test]
    fn selects_canon_port_with_ptp_phones_in_either_detection_order() {
        let canon = device("Canon EOS 650D", "usb:032,029");
        let phone = device("Apple iPhone 5 (PTP mode)", "usb:032,027");
        for devices in [
            vec![canon.clone(), phone.clone()],
            vec![phone, canon.clone()],
        ] {
            assert_eq!(select_t4i(&devices).unwrap(), canon);
        }
        // A reconnect must use the freshly detected address, not the previous USB port.
        let reconnected = device("Canon EOS 650D", "usb:032,030");
        assert_eq!(select_t4i(&[reconnected.clone()]).unwrap(), reconnected);
    }

    #[test]
    fn never_opens_an_unrelated_phone_or_a_different_camera_as_fallback() {
        for devices in [
            vec![],
            vec![device("Apple iPhone 5 (PTP mode)", "usb:032,027")],
            vec![device("Canon EOS 5D Mark III", "usb:032,028")],
        ] {
            let error = select_t4i(&devices).unwrap_err();
            assert_eq!(error.kind, CameraErrorKind::NotFound);
            assert!(error.message.contains("Canon T4i / 650D"));
        }
    }

    #[test]
    fn requires_one_physical_t4i_but_allows_aliases_on_the_same_port() {
        let canon = device("Canon EOS 650D", "usb:032,029");
        let alias = device("Canon Rebel T4i", "usb:032,029");
        assert_eq!(
            select_t4i(&[canon.clone(), alias.clone()]).unwrap().1,
            canon.1
        );
        assert_eq!(select_t4i(&[alias.clone()]).unwrap(), alias);
        let second = device("Canon Rebel T4i", "usb:032,031");
        assert_eq!(
            select_t4i(&[canon, second]).unwrap_err().kind,
            CameraErrorKind::Invalid
        );
    }

    #[test]
    fn rejects_stale_choice_and_wrong_type() {
        let p = CameraParameter {
            id: "iso".into(),
            label: "ISO".into(),
            value: ParameterValue::Choice("100".into()),
            kind: ParameterKind::Choice(vec!["100".into(), "200".into()]),
            read_only: false,
        };
        assert!(validate_parameter(&p, &ParameterValue::Choice("200".into())).is_ok());
        assert!(validate_parameter(&p, &ParameterValue::Choice("12800".into())).is_err());
        assert!(validate_parameter(&p, &ParameterValue::Number(200.0)).is_err());
    }
    #[test]
    fn c_abi_file_path_has_expected_layout() {
        assert_eq!(std::mem::size_of::<FilePath>(), 1152);
        assert_eq!(std::mem::offset_of!(FilePath, folder), 128);
    }
    #[test]
    #[ignore = "requires a local libgphoto2 installation; opens no camera session"]
    fn runtime_library_loads_without_connecting() {
        let backend = GPhoto::load().expect("load all C API symbols and allocate a context");
        assert!(backend.camera.is_null());
    }
}
