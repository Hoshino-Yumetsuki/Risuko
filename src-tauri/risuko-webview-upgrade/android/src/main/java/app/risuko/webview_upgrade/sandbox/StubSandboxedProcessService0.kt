package app.risuko.webview_upgrade.sandbox

import android.app.Service
import android.content.Intent
import android.os.IBinder

/**
 * Stand-in renderer service (slot 0). Runs in its own normal child process and
 * delegates everything to [SandboxedProcessServiceDelegate], which loads and
 * drives the swapped WebView kernel's real sandboxed service. The
 * [android.os.IBinder] hand-off is what links Chromium's browser process to
 * this renderer
 */
class StubSandboxedProcessService0 : Service() {
    private val delegate = SandboxedProcessServiceDelegate()
    override fun onCreate() {
        super.onCreate()
        delegate.onCreate(this, applicationContext)
    }
    override fun onBind(intent: Intent): IBinder? = delegate.onBind(intent)
    override fun onRebind(intent: Intent) {
        super.onRebind(intent)
        delegate.onRebind(intent)
    }
    override fun onDestroy() {
        super.onDestroy()
        delegate.onDestroy()
    }
}
