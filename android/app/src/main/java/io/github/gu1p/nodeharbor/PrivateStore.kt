package io.github.gu1p.nodeharbor

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** One instance, owned by the application process; the emulator receives no path. */
class PrivateStore(private val directory: File, private val alias: String = "nodeharbor-enrollment-v1") {
    private val settings = AtomicFile(directory.resolve("settings.json"))
    private val credentials = AtomicFile(directory.resolve("credentials.bin"))
    init { check(directory.isDirectory || directory.mkdirs()) { "Private storage is unavailable" } }

    @Synchronized fun load(): StoredState {
        if (!settings.baseFile.exists() && !File(settings.baseFile.path + ".bak").exists()) return StoredState()
        val bytes = settings.openRead().use { it.readNBytes(1024 * 1024 + 1) }
        require(bytes.size <= 1024 * 1024) { "Saved settings exceed their supported size" }
        return StoredState.decode(bytes.toString(Charsets.UTF_8), BuildConfig.VERSION_CODE)
    }
    @Synchronized fun update(change: (StoredState) -> StoredState): StoredState {
        val changed = change(load())
        write(settings, changed.encode().toByteArray(Charsets.UTF_8))
        return changed
    }
    @Synchronized fun saveToken(token: String) {
        require(token.isNotEmpty() && token.length <= 16 * 1024) { "Invalid enrollment credential" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(alias.toByteArray(Charsets.UTF_8))
        check(cipher.iv.size == 12)
        write(credentials, byteArrayOf(1) + cipher.iv + cipher.doFinal(token.toByteArray(Charsets.UTF_8)))
    }
    @Synchronized fun token(): String? {
        if (!credentials.baseFile.exists() && !File(credentials.baseFile.path + ".bak").exists()) return null
        val bytes = credentials.openRead().use { it.readNBytes(32 * 1024 + 1) }
        require(bytes.size in 30..(32 * 1024) && bytes[0] == 1.toByte()) { "The saved enrollment credential could not be read" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(1, 13)))
        cipher.updateAAD(alias.toByteArray(Charsets.UTF_8))
        return cipher.doFinal(bytes.copyOfRange(13, bytes.size)).toString(Charsets.UTF_8)
    }
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val existing = store.getKey(alias, null)
        if (existing != null) return existing as SecretKey
        check(!credentials.baseFile.exists()) { "The enrollment encryption key is unavailable; reconnect to your fleet" }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256).build())
        return generator.generateKey()
    }
    private fun write(file: AtomicFile, bytes: ByteArray) {
        val output = file.startWrite()
        try { output.write(bytes); file.finishWrite(output) }
        catch (error: Exception) { file.failWrite(output); throw error }
    }
}
