package io.github.gu1p.nodeharbor;

import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import io.github.gu1p.nodeharbor.ISocketBroker;

interface ISandbox {
    Bundle inspect(String privatePath);
    void bootProbe(in ParcelFileDescriptor kernel, in ParcelFileDescriptor initrd, in ParcelFileDescriptor console);
    void boot(in ParcelFileDescriptor disk, in ParcelFileDescriptor kernel, in ParcelFileDescriptor initrd,
        in ParcelFileDescriptor seed, in ParcelFileDescriptor console, in ParcelFileDescriptor control,
        in ParcelFileDescriptor diskReader, ISocketBroker broker, int cpus, int memoryMib);
    void stop();
    String networkProbe(ISocketBroker broker, in byte[] address);
    boolean guestNetworkProbe(ISocketBroker broker, in byte[] dns);
}
