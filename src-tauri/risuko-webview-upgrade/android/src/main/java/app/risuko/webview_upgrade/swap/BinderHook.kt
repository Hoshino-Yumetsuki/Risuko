package app.risuko.webview_upgrade.swap

import android.os.IBinder

internal abstract class BinderHook {

    private val lock = Any()
    private var originalBinder: IBinder? = null
    protected var proxyBinder: ProxyBinder? = null
        private set
    private var currentlyHooked = false
    private var everHooked = false

    fun hook() = synchronized(lock) {
        if (currentlyHooked) return
        if (everHooked) {
            onProxyBinderReplace(proxyBinder!!)
        } else {
            val original = onTargetBinderObtain()
            val proxy = onProxyBinderCreate(original)
            onProxyBinderReplace(proxy)
            originalBinder = original
            proxyBinder = proxy
            everHooked = true
        }
        currentlyHooked = true
    }

    fun restore(): Boolean = synchronized(lock) {
        if (!currentlyHooked) return false
        onTargetBinderRestore(originalBinder!!)
        currentlyHooked = false
        true
    }

    protected abstract fun onTargetBinderObtain(): IBinder
    protected abstract fun onProxyBinderCreate(binder: IBinder): ProxyBinder
    protected abstract fun onTargetBinderRestore(binder: IBinder)
    protected abstract fun onProxyBinderReplace(binder: ProxyBinder)
}
