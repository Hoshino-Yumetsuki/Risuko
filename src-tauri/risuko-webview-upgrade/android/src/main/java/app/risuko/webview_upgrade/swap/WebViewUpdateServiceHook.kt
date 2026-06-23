package app.risuko.webview_upgrade.swap

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.IBinder
import android.os.IInterface
import android.os.Parcel
import android.os.Parcelable
import app.risuko.webview_upgrade.reflect.Reflect

internal class WebViewUpdateServiceHook(
    private val context: Context,
    private val webViewPackageName: String,
) : BinderHook() {

    private lateinit var binderCache: MutableMap<String, IBinder>

    private val interceptors: Map<String, Reflect.Interceptor> = mapOf(
        "waitForAndGetProvider" to Reflect.Interceptor { _, original ->
            rewriteProviderResponse(original())
        },
        "asBinder" to Reflect.Interceptor { _, original ->
            proxyBinder ?: original()
        },
        "isMultiProcessEnabled" to Reflect.Interceptor { _, _ -> true },
    )

    private fun rewriteProviderResponse(original: Any?): Any? {
        val response = original ?: return null
        val packageInfo = try {
            context.packageManager.getPackageInfo(
                webViewPackageName,
                PackageManager.GET_SHARED_LIBRARY_FILES
                    or PackageManager.GET_SIGNATURES
                    or PackageManager.GET_META_DATA,
            )
        } catch (e: PackageManager.NameNotFoundException) {
            throw RuntimeException(e)
        }
        Framework.setResponsePackageInfo(response, packageInfo)

        val parcel = Parcel.obtain()
        return try {
            parcel.writeParcelable(response as Parcelable, 0)
            parcel.setDataPosition(parcel.dataSize() - 4)
            parcel.writeInt(0)
            parcel.setDataPosition(0)
            val loader = response.javaClass.classLoader
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                parcel.readParcelable(loader, response.javaClass)
            } else {
                @Suppress("DEPRECATION")
                parcel.readParcelable(loader)
            }
        } finally {
            parcel.recycle()
        }
    }

    override fun onTargetBinderObtain(): IBinder =
        Framework.getService(Framework.SERVICE_WEBVIEW_UPDATE)
            ?: throw IllegalStateException("webviewupdate service unavailable")

    override fun onProxyBinderCreate(binder: IBinder): ProxyBinder {
        val realInterface: IInterface = Framework.webViewUpdateAsInterface(binder)
        val proxyInterface = Reflect.newInterfaceProxy(realInterface, interceptors) as IInterface
        binderCache = Framework.serviceCache()
        return ProxyBinder(realInterface.asBinder(), proxyInterface)
    }

    override fun onTargetBinderRestore(binder: IBinder) {
        binderCache[Framework.SERVICE_WEBVIEW_UPDATE] = binder
    }

    override fun onProxyBinderReplace(binder: ProxyBinder) {
        binderCache[Framework.SERVICE_WEBVIEW_UPDATE] = binder
    }
}
