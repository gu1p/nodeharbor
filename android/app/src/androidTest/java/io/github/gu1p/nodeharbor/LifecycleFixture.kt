package io.github.gu1p.nodeharbor

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.json.JSONObject

/** Only the unenrolled instrumentation guest gets this fixed disk-marker probe. */
internal fun installLifecycleMarker(guest: OwnedGuest, owner: String, fault: String? = null) {
    check(guest.directory.name.startsWith("owned-vm-contract-"))
    check(guest.receipt(owner) != null)
    val image = guest.file("seed.iso").readBytes()
    fun little(offset: Int) = ByteBuffer.wrap(image, offset, 4).order(ByteOrder.LITTLE_ENDIAN).int
    var offset = little(16 * 2048 + 158) * 2048
    val files = mutableMapOf<String, ByteArray>()
    while (image[offset] != 0.toByte()) {
        val length = image[offset].toInt() and 255
        if (image[offset + 25].toInt() and 2 == 0) {
            val nameLength = image[offset + 32].toInt() and 255
            val name = image.copyOfRange(offset + 33, offset + 33 + nameLength).toString(Charsets.US_ASCII).substringBefore(';').lowercase()
            val start = little(offset + 2) * 2048
            files[name] = image.copyOfRange(start, start + little(offset + 10))
        }
        offset += length
    }
    // Cloud-init caches per-instance completion across A -> B -> A changes.
    // Every test seed application must use a fresh identity, including recovery.
    val application = java.util.UUID.randomUUID().toString()
    files["meta-data"] = (files.getValue("meta-data").toString(Charsets.UTF_8).replace("\nlocal-hostname", "-fixture-${fault ?: "normal"}-$application\nlocal-hostname")).toByteArray()
    if (fault == "legacy") files["meta-data"] = files.getValue("meta-data").toString(Charsets.UTF_8)
        .replace("-control-$GUEST_CONTROL_REVISION", "-control-2").toByteArray()
    val config = JSONObject(files.getValue("user-data").toString(Charsets.UTF_8).substringAfter("#cloud-config\n"))
    val writes = config.getJSONArray("write_files")
    for (index in 0 until writes.length()) {
        val entry = writes.getJSONObject(index)
        // Reproduce revision 2's installed units as well as its protocol identity.
        // Recovery must update this existing filesystem through cloud-init.
        if (fault == "legacy" && entry.getString("path").endsWith("/nodeharbor-control.service"))
            entry.put("content", "[Unit]\nDescription=NodeHarbor private owner channel\nDefaultDependencies=no\nRequires=sysinit.target\nAfter=sysinit.target\nBefore=basic.target shutdown.target\nConflicts=shutdown.target\n[Service]\nType=notify\nStandardOutput=journal+console\nExecStart=/usr/bin/python3 /usr/local/lib/nodeharbor/android_control.py\nRestart=on-failure\nRestartSec=5\nTimeoutStartSec=120\nTimeoutStopSec=10\n[Install]\nWantedBy=multi-user.target\n")
        if (fault == "legacy" && entry.getString("path").endsWith("/nodeharbor-watchdog.timer"))
            entry.put("content", "[Unit]\nDescription=Check the NodeHarbor owner lease regularly\nWants=nodeharbor-control.service\nAfter=nodeharbor-control.service\n[Timer]\nOnActiveSec=15s\nOnUnitActiveSec=15s\nAccuracySec=1s\n[Install]\nWantedBy=timers.target\n")
        if (entry.getString("path").endsWith("/android_control.py")) {
            val marker = """

_original_status = Guest.status
def _test_crash_once():
    token = ROOT / 'lifecycle-test-crash-fired'
    try: descriptor = os.open(token, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError: return
    try: os.fsync(descriptor)
    finally: os.close(descriptor)
    directory = os.open(str(ROOT), os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(directory)
    finally: os.close(directory)
    # Recovery can answer using the old script before cloud-init refreshes it.
    # Persist consumption before panicking so that boot can apply its new seed.
    threading.Timer(2, lambda: (Path('/proc/sys/kernel/panic').write_text('0'), Path('/proc/sysrq-trigger').write_text('c'))).start()

def _marker_status(self):
    marker = ROOT / 'lifecycle-test-marker'
    expected = b'previously flushed lifecycle disk marker\n'
    if marker.exists():
        if marker.read_bytes() != expected: raise ValueError('The lifecycle disk marker changed')
        event = 'marker-preserved'
    else:
        with marker.open('wb') as output:
            output.write(expected)
            output.flush()
            os.fsync(output.fileno())
        directory = os.open(str(ROOT), os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(directory)
        finally: os.close(directory)
        event = 'marker-created'
    print('NODEHARBOR_TEST ' + event, flush=True)
    print('NODEHARBOR_TEST mode-${fault ?: "normal"}', flush=True)
    # Leave this test kernel panicked so the owner must force its teardown.
    ${if (fault == "crash") "_test_crash_once()" else ""}
    return _original_status(self)
Guest.status = _marker_status

"""
            var source = entry.getString("content").replace("if __name__ == '__main__': main()", marker + "if __name__ == '__main__': main()")
            if (fault == "legacy") source = source.replace("CONTROL_REVISION = '$GUEST_CONTROL_REVISION'", "CONTROL_REVISION = '2'")
                .replace("'controlRevision': control_revision()", "'controlRevision': CONTROL_REVISION")
            if (fault == "unresponsive") source = source.replace("command = request['command']", "command = request['command']\n            if command == 'poweroff': threading.Event().wait()")
            entry.put("content", source)
        }
    }
    files["user-data"] = "#cloud-config\n$config\n".toByteArray()
    guest.file("seed.iso").writeBytes(seedImage(files))
}
