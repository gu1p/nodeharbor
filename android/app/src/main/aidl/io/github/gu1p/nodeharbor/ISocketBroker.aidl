package io.github.gu1p.nodeharbor;
import android.os.ParcelFileDescriptor;

interface ISocketBroker {
    ParcelFileDescriptor openTcp(int handle, in byte[] address, int port);
    ParcelFileDescriptor openUdp(int handle);
    int sendUdp(int handle, in byte[] address, int port, in byte[] payload);
    void closeFlow(int handle);
    int localPort(int handle);
}
