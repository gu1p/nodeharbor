package io.github.gu1p.nodeharbor

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.json.JSONObject

/** Fixed data probe, only in the unenrolled instrumentation guest's private seed. */
internal fun installPoolFixture(guest: OwnedGuest, owner: String) {
    check(guest.directory.name.startsWith("owned-vm-contract-pool-"))
    installLifecycleMarker(guest, owner)
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
    val config = JSONObject(files.getValue("user-data").toString(Charsets.UTF_8).substringAfter("#cloud-config\n"))
    val writes = config.getJSONArray("write_files")
    for (index in 0 until writes.length()) {
        val entry = writes.getJSONObject(index)
        if (entry.getString("path").endsWith("/android_storage.py")) {
            val fixture = """

_fixture_operate = operate
def _fixture_storage(request):
    result = _fixture_operate(request)
    if request['action'] == 'apply':
        marker = Path('/var/lib/nodeharbor/storage/android-pool-test-marker')
        expected = b'previously flushed Android pool data\n'
        created = not marker.exists()
        if created:
            with marker.open('xb') as output:
                output.write(expected)
                output.flush()
                os.fsync(output.fileno())
            descriptor = os.open(marker.parent, os.O_RDONLY | os.O_DIRECTORY)
            try: os.fsync(descriptor)
            finally: os.close(descriptor)
        if marker.read_bytes() != expected: raise ValueError('The pool marker changed')
        result['testMarkerCreated'] = created
    return result
operate = _fixture_storage

"""
            val source = entry.getString("content")
            check(source.contains("if __name__ == '__main__': main()"))
            entry.put("content", source.replace("if __name__ == '__main__': main()", fixture + "if __name__ == '__main__': main()"))
        }
        if (entry.getString("path").endsWith("/android_control.py")) {
            // No enrollment or bootstrap grant is issued in this fixture. Keep
            // storage failures in the bounded private console for diagnosis.
            entry.put("content", entry.getString("content").replace("stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as child:",
                "stdout=subprocess.PIPE, stderr=None) as child:"))
        }
    }
    files["user-data"] = "#cloud-config\n$config\n".toByteArray()
    guest.file("seed.iso").writeBytes(seedImage(files))
}
