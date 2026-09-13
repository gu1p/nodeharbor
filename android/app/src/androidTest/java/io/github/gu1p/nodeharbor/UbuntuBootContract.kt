package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID

class UbuntuBootContract {
    @Test fun packagedRuntimeBootsUbuntuAndHttpsThroughItsIsolatedNetwork() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = context.filesDir.resolve("ubuntu-probe")
        for (name in listOf("root.img", "kernel", "initrd")) assertTrue("Provision the verified Ubuntu probe $name", directory.resolve(name).isFile)
        val script = "import platform,urllib.request; assert platform.machine() == 'aarch64'; " +
            "r=urllib.request.urlopen('https://example.com', timeout=60); assert r.status == 200; " +
            "open('/dev/ttyAMA0','w').write('NODEHARBOR_UBUNTU_HTTPS_OK\\n')"
        val config = JSONObject().put("users", JSONArray()).put("ssh_pwauth", false).put("disable_root", true)
            .put("package_update", false).put("package_upgrade", false)
            .put("runcmd", JSONArray().put(JSONArray(listOf("python3", "-c", script))).put(JSONArray(listOf("systemctl", "poweroff"))))
        val seed = directory.resolve("seed.iso")
        seed.writeBytes(seedImage(mapOf(
            "meta-data" to "instance-id: nodeharbor-test-${UUID.randomUUID()}\nlocal-hostname: nodeharbor-guest\n".toByteArray(),
            "user-data" to "#cloud-config\n$config\n".toByteArray(),
            "network-config" to "version: 2\nethernets:\n  worker:\n    match:\n      name: 'e*'\n    dhcp4: true\n    dhcp6: false\n    nameservers:\n      addresses: [10.0.2.3]\n".toByteArray()
        )))
        val renewal = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()
        val console = WorkerWakeLock(context).use { wake ->
            renewal.scheduleAtFixedRate({ wake.update(true) }, 0, 30, java.util.concurrent.TimeUnit.SECONDS)
            try { SandboxClient(context).bootGuestProbe(directory.resolve("root.img"), directory.resolve("kernel"), directory.resolve("initrd"), seed) }
            finally { renewal.shutdownNow() }
        }
        assertTrue("Ubuntu did not complete its real HTTPS contract:\n${console.takeLast(10000)}", console.contains("NODEHARBOR_UBUNTU_HTTPS_OK"))
    }
}
