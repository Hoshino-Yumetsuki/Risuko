package app.risuko.webview_upgrade.sandbox

/** Intent-extra keys the [ActivityManagerHook] sets and the delegate reads */
internal object SandboxExtras {
    const val WEBVIEW_APK_PATH = "EXTRA_WEBVIEW_APK_PATH"
    const val WEBVIEW_LIBS_PATH = "EXTRA_WEBVIEW_LIBS_PATH"
    const val WEBVIEW_PACKAGE_NAME = "EXTRA_WEBVIEW_PACKAGE_NAME"
    const val ORIGINAL_SERVICE_CLASS = "EXTRA_ORIGINAL_SERVICE_CLASS"
    const val SHARED_LIBS = "EXTRA_WEBVIEW_SHARED_LIBS"

    /** Chromium's own browser-package extra */
    const val CHROMIUM_BROWSER_PACKAGE =
        "org.chromium.base.process_launcher.extra.browser_package_name"
}
