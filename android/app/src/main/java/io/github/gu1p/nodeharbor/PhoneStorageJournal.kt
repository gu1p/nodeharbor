package io.github.gu1p.nodeharbor

import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

data class PhoneStorageChange(val id: String, val previous: PhoneStoragePool?, val target: PhoneStoragePool,
    val requiresBackup: Boolean, val phase: String) {
    fun json(): JSONObject = JSONObject().put("id", id).put("previous", previous?.let { JSONObject(it.encode()) } ?: JSONObject.NULL)
        .put("target", JSONObject(target.encode())).put("requiresBackup", requiresBackup).put("phase", phase)
    companion object {
        fun from(value: JSONObject, owner: String): PhoneStorageChange {
            val id = value.getString("id")
            require(UUID.fromString(id).toString() == id)
            val previous = if (value.isNull("previous")) null else PhoneStoragePool.decode(value.getJSONObject("previous").toString(), owner)
            val target = PhoneStoragePool.decode(value.getJSONObject("target").toString(), owner)
            val phase = value.getString("phase")
            require(phase in setOf("planned", "backed-up", "files-ready", "verified")) { "Unknown storage journal stage; files were preserved" }
            require(target.generation > (previous?.generation ?: 0)) { "Storage change has a stale generation" }
            return PhoneStorageChange(id, previous, target, value.getBoolean("requiresBackup"), phase)
        }
    }
}

data class PhoneStorageBackup(val operation: String, val poolId: String)
data class PhoneStorageRecovery(val operation: String, val replacement: List<PhoneStorageDisk>, val phase: String) {
    fun json(): JSONObject = JSONObject().put("operation", operation).put("replacement", JSONArray(replacement.map { it.json() })).put("phase", phase)
    companion object {
        fun from(value: JSONObject): PhoneStorageRecovery {
            val operation = value.getString("operation")
            require(UUID.fromString(operation).toString() == operation)
            val entries = value.getJSONArray("replacement")
            require(entries.length() in 1..16)
            val disks = (0 until entries.length()).map { PhoneStorageDisk.from(entries.getJSONObject(it)) }
            require(disks.sumOf { it.gib.toLong() } in 15L..65536L && disks.map { it.id }.distinct().size == disks.size)
            val phase = value.getString("phase")
            require(phase in setOf("resetting", "rebuilding")) { "Unknown storage recovery stage; files were preserved" }
            return PhoneStorageRecovery(operation, disks, phase)
        }
    }
}

data class PhoneStorageState(val deviceId: String, val pool: PhoneStoragePool? = null, val generation: Long = 0,
    val pending: PhoneStorageChange? = null, val retained: List<PhoneStorageDisk> = emptyList(),
    val disabled: Boolean = false, val automaticRecovery: Boolean = false, val backups: List<PhoneStorageBackup> = emptyList(),
    val recovery: PhoneStorageRecovery? = null) {
    val ready: Boolean get() = !disabled && pending == null && recovery == null
    fun encode(): String = JSONObject().put("format", 1).put("deviceId", deviceId).put("generation", generation)
        .put("pool", pool?.let { JSONObject(it.encode()) } ?: JSONObject.NULL)
        .put("pending", pending?.json() ?: JSONObject.NULL).put("retained", JSONArray(retained.map { it.json() }))
        .put("disabled", disabled).put("automaticRecovery", automaticRecovery)
        .put("backups", JSONArray(backups.map { JSONObject().put("operation", it.operation).put("poolId", it.poolId) }))
        .put("recovery", recovery?.json() ?: JSONObject.NULL).toString()
    fun commit(confirmedStopped: Boolean): PhoneStorageState {
        val change = checkNotNull(pending) { "No storage change is pending" }
        check(confirmedStopped && change.phase == "verified") { "Confirm guest storage verification and Android process death before committing the change" }
        check(pool == change.previous) { "The original storage pool changed; its files were preserved" }
        val originals = change.previous?.disks.orEmpty().filter { old -> change.target.disks.none { it == old } }
        return copy(pool = change.target, generation = change.target.generation, pending = null,
            retained = (retained + originals).distinctBy { it.location to it.filename }, disabled = false,
            backups = backups + if (change.requiresBackup) listOf(PhoneStorageBackup(change.id, checkNotNull(change.previous).poolId)) else emptyList())
    }
    companion object {
        fun decode(text: String, owner: String): PhoneStorageState {
            require(text.length <= 1024 * 1024)
            val value = JSONObject(text)
            require(value.getInt("format") == 1 && value.getString("deviceId") == owner && UUID.fromString(owner).toString() == owner)
            val pool = if (value.isNull("pool")) null else PhoneStoragePool.decode(value.getJSONObject("pool").toString(), owner)
            val pending = if (value.isNull("pending")) null else PhoneStorageChange.from(value.getJSONObject("pending"), owner)
            val generation = value.getLong("generation")
            require(generation >= 0 && (pool == null || pool.generation == generation))
            require(pending == null || pending.previous == pool && pending.target.generation > generation)
            val retained = value.getJSONArray("retained")
            require(retained.length() <= 256) { "Review retained storage copies before creating more" }
            val backupArray = value.optJSONArray("backups") ?: JSONArray()
            require(backupArray.length() <= 256)
            val backups = (0 until backupArray.length()).map {
                val entry = backupArray.getJSONObject(it)
                PhoneStorageBackup(entry.getString("operation"), entry.getString("poolId")).also { backup ->
                    require(UUID.fromString(backup.operation).toString() == backup.operation && UUID.fromString(backup.poolId).toString() == backup.poolId)
                }
            }
            return PhoneStorageState(owner, pool, generation, pending,
                (0 until retained.length()).map { PhoneStorageDisk.from(retained.getJSONObject(it)) },
                value.getBoolean("disabled"), value.optBoolean("automaticRecovery", false), backups,
                value.optJSONObject("recovery")?.let(PhoneStorageRecovery::from))
        }
    }
}
