//! Android `Intent` helpers for opening files and folders
//!
//! `tauri-plugin-shell::open` only constructs `Intent(ACTION_VIEW, uri)`
//! without an explicit MIME type or any extras. On Android, that is
//! enough on a real device with a file manager (Files by Google,
//! Material Files, etc.) preinstalled — the system narrows candidates
//! by querying the documents provider's MIME and dispatches to the
//! right viewer. But on the AOSP emulator or stripped-down Android
//! builds, the Messages app's `mimeType="*/*"` filter wins because no
//! other app advertises a more specific match
//!
//! We build the `Intent` ourselves via JNI:
//!   - We attach the file's actual MIME (e.g. `video/mp4`) via
//!     `Intent.setDataAndType`, which narrows the candidate set
//!   - We wrap the result in `Intent.createChooser` so even when several
//!     apps still match, the user gets the standard "Open with" sheet
//!     (the behavior the user expects from Android)
//!   - We add `FLAG_GRANT_READ_URI_PERMISSION` so the receiving app can
//!     read the URI we built ourselves (without this, our hand-built
//!     externalstorage URIs come through as 0-byte files in the viewer)
//!
//! ## JNI bootstrap
//!
//! Tauri 2 mobile does not initialize `ndk-context` on this path
//! We capture the `JavaVM` in `JNI_OnLoad`, keep it in a `OnceLock`, then attach threads as needed
//! When Rust needs a context, we resolve the current `Application` through `ActivityThread`

#![cfg(target_os = "android")]

use std::collections::HashMap;
use std::os::raw::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, jstring, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM};

const FLAG_ACTIVITY_NEW_TASK: i32 = 0x10000000;
const FLAG_GRANT_READ_URI_PERMISSION: i32 = 0x00000001;

static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();
static DIRECTORY_PICKERS: OnceLock<
    Mutex<HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>>,
> = OnceLock::new();
static DIRECTORY_PICKER_COUNTER: AtomicU64 = AtomicU64::new(1);

/// JNI entry point called when Android loads `libapp_lib.so`
/// Store the `JavaVM` so background Rust threads can call back into Kotlin
///
/// `#[no_mangle]` keeps the symbol name visible to the dynamic linker
/// Return `JNI_VERSION_1_6`, the lowest version we need
#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: *mut jni::sys::JavaVM, _: *mut c_void) -> jint {
    // SAFETY: Android gives us a process-lifetime `JavaVM*`
    if let Ok(vm) = unsafe { JavaVM::from_raw(vm) } {
        let _ = JAVA_VM.set(vm);
    }
    JNI_VERSION_1_6
}

#[no_mangle]
pub extern "system" fn Java_app_risuko_mobile_MainActivity_nativeOnDirectoryPicked(
    mut env: JNIEnv,
    _activity: JObject,
    request_id: jstring,
    uri: jstring,
) {
    let request_id = if request_id.is_null() {
        String::new()
    } else {
        let request_id = unsafe { JString::from_raw(request_id) };
        env.get_string(&request_id)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let uri = if uri.is_null() {
        None
    } else {
        let uri = unsafe { JString::from_raw(uri) };
        env.get_string(&uri)
            .map(|s| s.to_string_lossy().into_owned())
            .ok()
    };
    if request_id.is_empty() {
        return;
    }
    if let Ok(mut pending) = directory_pickers().lock() {
        if let Some(tx) = pending.remove(&request_id) {
            let _ = tx.send(uri);
        }
    }
}

fn directory_pickers(
) -> &'static Mutex<HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>> {
    DIRECTORY_PICKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn pick_directory() -> Result<Option<String>, String> {
    let request_id = format!(
        "{}-{}",
        std::process::id(),
        DIRECTORY_PICKER_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    directory_pickers()
        .lock()
        .map_err(|e| format!("directory picker lock poisoned: {e}"))?
        .insert(request_id.clone(), tx);

    if let Err(err) = start_directory_picker(&request_id) {
        if let Ok(mut pending) = directory_pickers().lock() {
            pending.remove(&request_id);
        }
        return Err(err);
    }

    match tokio::time::timeout(Duration::from_secs(300), rx).await {
        Ok(Ok(uri)) => Ok(uri),
        Ok(Err(_)) => Err("Android directory picker was interrupted".to_string()),
        Err(_) => {
            if let Ok(mut pending) = directory_pickers().lock() {
                pending.remove(&request_id);
            }
            Err("Android directory picker timed out".to_string())
        }
    }
}

fn start_directory_picker(request_id: &str) -> Result<(), String> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| "JavaVM not captured (JNI_OnLoad didn't run?)".to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach thread: {e}"))?;
    let activity = main_activity_class(&mut env)?;
    let request_id = env
        .new_string(request_id)
        .map_err(|e| format!("new_string request_id: {e}"))?;
    let started = env
        .call_static_method(
            activity,
            "pickDirectory",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&request_id)],
        )
        .map_err(|e| format!("MainActivity.pickDirectory: {e}"))?
        .z()
        .map_err(|e| format!("pickDirectory result: {e}"))?;
    if started {
        Ok(())
    } else {
        Err("Android activity is not ready".to_string())
    }
}

pub fn set_system_bars(dark_mode: bool) -> Result<(), String> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| "JavaVM not captured (JNI_OnLoad didn't run?)".to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach thread: {e}"))?;
    let activity = main_activity_class(&mut env)?;
    env.call_static_method(
        activity,
        "setSystemBars",
        "(Z)V",
        &[JValue::Bool(if dark_mode { 1 } else { 0 })],
    )
    .map_err(|e| format!("MainActivity.setSystemBars: {e}"))?;
    Ok(())
}

pub fn ensure_all_files_access() -> Result<bool, String> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| "JavaVM not captured (JNI_OnLoad didn't run?)".to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach thread: {e}"))?;
    let activity = main_activity_class(&mut env)?;
    let granted = env
        .call_static_method(activity, "requestAllFilesAccess", "()Z", &[])
        .map_err(|e| format!("MainActivity.requestAllFilesAccess: {e}"))?
        .z()
        .map_err(|e| format!("requestAllFilesAccess result: {e}"))?;
    Ok(granted)
}

pub fn show_download_notification(
    progress: u32,
    active_count: u32,
    detail: &str,
) -> Result<(), String> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| "JavaVM not captured (JNI_OnLoad didn't run?)".to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach thread: {e}"))?;
    let activity = main_activity_class(&mut env)?;
    let detail = env
        .new_string(detail)
        .map_err(|e| format!("new_string detail: {e}"))?;
    env.call_static_method(
        activity,
        "showDownloadNotification",
        "(IILjava/lang/String;)V",
        &[
            JValue::Int(progress.min(100) as i32),
            JValue::Int(active_count.min(i32::MAX as u32) as i32),
            JValue::Object(&detail),
        ],
    )
    .map_err(|e| format!("MainActivity.showDownloadNotification: {e}"))?;
    Ok(())
}

pub fn hide_download_notification() -> Result<(), String> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| "JavaVM not captured (JNI_OnLoad didn't run?)".to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach thread: {e}"))?;
    let activity = main_activity_class(&mut env)?;
    env.call_static_method(activity, "hideDownloadNotification", "()V", &[])
        .map_err(|e| format!("MainActivity.hideDownloadNotification: {e}"))?;
    Ok(())
}

/// Open `path` in a system file manager via `MainActivity.revealFolder`
///
/// The Kotlin helper tries several intent shapes in order: a chooser with
/// `vnd.android.document/directory` MIME, direct dispatch with the same
/// MIME, then direct dispatch with no MIME. Each attempt logs to logcat
/// under the `RisukoReveal` tag so we can trace whatever the device did.
/// Returns `"ok"` on success, or a diagnostic string we pass back
/// verbatim. The renderer already logs the error and shows a localized
/// toast
pub fn reveal_folder(path: &str) -> Result<(), String> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| "JavaVM not captured (JNI_OnLoad didn't run?)".to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach thread: {e}"))?;
    let activity = main_activity_class(&mut env)?;
    let path_str = env
        .new_string(path)
        .map_err(|e| format!("new_string path: {e}"))?;
    let value = env
        .call_static_method(
            activity,
            "revealFolder",
            "(Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&path_str)],
        )
        .map_err(|e| format!("MainActivity.revealFolder: {e}"))?
        .l()
        .map_err(|e| format!("revealFolder result not object: {e}"))?;
    if value.is_null() {
        return Err("revealFolder returned null".to_string());
    }
    let value_str = JString::from(value);
    let outcome = env
        .get_string(&value_str)
        .map_err(|e| format!("get_string revealFolder: {e}"))?
        .to_string_lossy()
        .into_owned();
    if outcome == "ok" {
        Ok(())
    } else {
        log::warn!("[Risuko] revealFolder({path}) -> {outcome}");
        Err(outcome)
    }
}

fn main_activity_class<'env>(env: &mut JNIEnv<'env>) -> Result<JClass<'env>, String> {
    let app = current_application(env).map_err(|e| format!("get application: {e}"))?;
    let class_loader = env
        .call_method(&app, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| format!("getClassLoader: {e}"))?
        .l()
        .map_err(|e| format!("classLoader not object: {e}"))?;
    let class_name = env
        .new_string("app.risuko.mobile.MainActivity")
        .map_err(|e| format!("new_string class_name: {e}"))?;
    let activity = env
        .call_method(
            class_loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&class_name)],
        )
        .map_err(|e| format!("load MainActivity: {e}"))?
        .l()
        .map_err(|e| format!("MainActivity class not object: {e}"))?;
    Ok(JClass::from(activity))
}

/// Resolve the current `Application` through `ActivityThread`
/// Background Rust threads do not hold an `Activity`, but the application context works with `FLAG_ACTIVITY_NEW_TASK`
fn current_application<'env>(env: &mut JNIEnv<'env>) -> jni::errors::Result<JObject<'env>> {
    let class = env.find_class("android/app/ActivityThread")?;
    let thread = env
        .call_static_method(
            class,
            "currentActivityThread",
            "()Landroid/app/ActivityThread;",
            &[],
        )?
        .l()?;
    let app = env
        .call_method(thread, "getApplication", "()Landroid/app/Application;", &[])?
        .l()?;
    Ok(app)
}

/// Dispatch `ACTION_VIEW` with an explicit MIME and an Android chooser
///
/// The `chooser_title` is shown at the top of the chooser sheet
/// (e.g. "Open file with"). `mime` should be a concrete MIME type
/// (`video/mp4`, `image/jpeg`, `vnd.android.document/directory`...)
pub fn dispatch_view_with_chooser(
    uri: &str,
    mime: &str,
    chooser_title: &str,
) -> Result<(), String> {
    dispatch_uri_with_chooser(uri, mime, chooser_title, uri.starts_with("content://"))
}

pub fn dispatch_file_path_with_chooser(
    path: &str,
    mime: &str,
    chooser_title: &str,
) -> Result<(), String> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| "JavaVM not captured (JNI_OnLoad didn't run?)".to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach thread: {e}"))?;

    let activity = current_application(&mut env).map_err(|e| format!("get application: {e}"))?;
    let authority = package_file_provider_authority(&mut env, &activity)
        .map_err(|e| format!("get file provider authority: {e}"))?;
    let path_str = env
        .new_string(path)
        .map_err(|e| format!("new_string path: {e}"))?;
    let authority_str = env
        .new_string(authority)
        .map_err(|e| format!("new_string authority: {e}"))?;
    let file_class = env
        .find_class("java/io/File")
        .map_err(|e| format!("find File: {e}"))?;
    let file = env
        .new_object(
            &file_class,
            "(Ljava/lang/String;)V",
            &[JValue::Object(&path_str)],
        )
        .map_err(|e| format!("new File: {e}"))?;
    let provider_class = env
        .find_class("androidx/core/content/FileProvider")
        .map_err(|e| format!("find FileProvider: {e}"))?;
    let uri = env
        .call_static_method(
            &provider_class,
            "getUriForFile",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/io/File;)Landroid/net/Uri;",
            &[
                JValue::Object(&activity),
                JValue::Object(&authority_str),
                JValue::Object(&file),
            ],
        )
        .map_err(|e| format!("FileProvider.getUriForFile: {e}"))?
        .l()
        .map_err(|e| format!("FileProvider URI not object: {e}"))?;

    dispatch_jni_uri_with_chooser(&mut env, activity, uri, mime, chooser_title, true)
}

fn dispatch_uri_with_chooser(
    uri: &str,
    mime: &str,
    chooser_title: &str,
    grant_read_permission: bool,
) -> Result<(), String> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| "JavaVM not captured (JNI_OnLoad didn't run?)".to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach thread: {e}"))?;

    let activity = current_application(&mut env).map_err(|e| format!("get application: {e}"))?;

    let uri_str = env
        .new_string(uri)
        .map_err(|e| format!("new_string uri: {e}"))?;
    let uri_class = env
        .find_class("android/net/Uri")
        .map_err(|e| format!("find Uri: {e}"))?;
    let parsed_uri = env
        .call_static_method(
            &uri_class,
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&uri_str)],
        )
        .map_err(|e| format!("Uri.parse: {e}"))?
        .l()
        .map_err(|e| format!("Uri.parse not object: {e}"))?;

    dispatch_jni_uri_with_chooser(
        &mut env,
        activity,
        parsed_uri,
        mime,
        chooser_title,
        grant_read_permission,
    )
}

fn dispatch_jni_uri_with_chooser(
    env: &mut JNIEnv,
    activity: JObject,
    parsed_uri: JObject,
    mime: &str,
    chooser_title: &str,
    grant_read_permission: bool,
) -> Result<(), String> {
    // Build the inner `Intent(ACTION_VIEW, uri)` with explicit MIME.
    let action = env
        .new_string("android.intent.action.VIEW")
        .map_err(|e| format!("new_string action: {e}"))?;
    let mime_str = env
        .new_string(mime)
        .map_err(|e| format!("new_string mime: {e}"))?;
    let intent_class = env
        .find_class("android/content/Intent")
        .map_err(|e| format!("find Intent: {e}"))?;
    let intent = env
        .new_object(
            &intent_class,
            "(Ljava/lang/String;)V",
            &[JValue::Object(&action)],
        )
        .map_err(|e| format!("new Intent: {e}"))?;

    env.call_method(
        &intent,
        "setDataAndType",
        "(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(&parsed_uri), JValue::Object(&mime_str)],
    )
    .map_err(|e| format!("setDataAndType: {e}"))?;

    if grant_read_permission {
        env.call_method(
            &intent,
            "addFlags",
            "(I)Landroid/content/Intent;",
            &[JValue::Int(FLAG_GRANT_READ_URI_PERMISSION)],
        )
        .map_err(|e| format!("addFlags grant: {e}"))?;
    }

    // Wrap in `Intent.createChooser(intent, title)` so the user sees the
    // standard "Open with" sheet even when a single app has been set as
    // default (matching desktop double-click semantics where the user can
    // change the handler at any time).
    let title = env
        .new_string(chooser_title)
        .map_err(|e| format!("new_string title: {e}"))?;
    let chooser = env
        .call_static_method(
            &intent_class,
            "createChooser",
            "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
            &[JValue::Object(&intent), JValue::Object(&title)],
        )
        .map_err(|e| format!("createChooser: {e}"))?
        .l()
        .map_err(|e| format!("createChooser not object: {e}"))?;

    // The chooser is started from an Activity that may not be in the
    // foreground task; mark it as a fresh task so Android grants the
    // necessary launch context.
    env.call_method(
        &chooser,
        "addFlags",
        "(I)Landroid/content/Intent;",
        &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)],
    )
    .map_err(|e| format!("addFlags new_task: {e}"))?;

    if grant_read_permission {
        env.call_method(
            &chooser,
            "addFlags",
            "(I)Landroid/content/Intent;",
            &[JValue::Int(FLAG_GRANT_READ_URI_PERMISSION)],
        )
        .map_err(|e| format!("addFlags chooser grant: {e}"))?;
    }

    env.call_method(
        &activity,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&chooser)],
    )
    .map_err(|e| format!("startActivity: {e}"))?;

    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err("startActivity threw".into());
    }

    Ok(())
}

fn package_file_provider_authority(
    env: &mut JNIEnv,
    activity: &JObject,
) -> jni::errors::Result<String> {
    let package_name = env
        .call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?
        .l()?;
    let package_name = env.get_string((&package_name).into())?;
    Ok(format!("{}.fileprovider", package_name.to_string_lossy()))
}
