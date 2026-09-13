package io.github.gu1p.nodeharbor

internal class SocketBridge(private val broker: ISocketBroker) {
    fun openTcp(handle: Int, address: ByteArray, port: Int): Int = broker.openTcp(handle, address, port).detachFd()
    fun openUdp(handle: Int): Int = broker.openUdp(handle).detachFd()
    fun sendUdp(handle: Int, address: ByteArray, port: Int, payload: ByteArray): Int = broker.sendUdp(handle, address, port, payload)
    fun closeFlow(handle: Int) = broker.closeFlow(handle)
    fun localPort(handle: Int): Int = broker.localPort(handle)
}

internal object NativeNetwork {
    init { System.loadLibrary("nodeharbor-network") }
    external fun run(descriptor: Int, dns: ByteArray, bridge: SocketBridge)
}
