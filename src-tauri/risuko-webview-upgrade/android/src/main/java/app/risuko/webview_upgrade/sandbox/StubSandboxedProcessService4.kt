package app.risuko.webview_upgrade.sandbox

import android.app.Service
import android.content.Intent
import android.os.IBinder

/** Stand-in renderer service (slot 4). See [StubSandboxedProcessService0] */
class StubSandboxedProcessService4 : Service() {
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
