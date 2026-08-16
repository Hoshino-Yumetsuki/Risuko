package app.risuko.webview_upgrade.swap

import android.os.IBinder
import android.os.IInterface
import android.os.Parcel
import java.io.FileDescriptor

internal class ProxyBinder(
    private val remote: IBinder,
    initialProxyInterface: IInterface,
) : IBinder {

    @Volatile
    private var proxyInterface: IInterface = initialProxyInterface

    val currentProxyInterface: IInterface get() = proxyInterface

    override fun queryLocalInterface(descriptor: String): IInterface = proxyInterface

    override fun getInterfaceDescriptor(): String? = remote.interfaceDescriptor
    override fun pingBinder(): Boolean = remote.pingBinder()
    override fun isBinderAlive(): Boolean = remote.isBinderAlive
    override fun dump(fd: FileDescriptor, args: Array<out String>?) = remote.dump(fd, args)
    override fun dumpAsync(fd: FileDescriptor, args: Array<out String>?) = remote.dumpAsync(fd, args)
    override fun transact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean =
        remote.transact(code, data, reply, flags)

    override fun linkToDeath(recipient: IBinder.DeathRecipient, flags: Int) =
        remote.linkToDeath(recipient, flags)

    override fun unlinkToDeath(recipient: IBinder.DeathRecipient, flags: Int): Boolean =
        remote.unlinkToDeath(recipient, flags)
}
