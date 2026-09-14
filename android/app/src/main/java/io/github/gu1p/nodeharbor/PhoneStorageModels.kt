package io.github.gu1p.nodeharbor

import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

const val STORAGE_GIB = 1024L * 1024 * 1024

data class PhoneStorageLocation(val id: String, val label: String, val filesystem: String,
    val freeBytes: Long, val available: Boolean, val removable: Boolean = false)

data class PhoneStorageDisk(val id: String, val location: String, val gib: Int, val incarnation: String) {
    val bytes: Long get() = gib * STORAGE_GIB
    val filename: String get() = "$id-$incarnation.img"
    fun json(): JSONObject = JSONObject().put("id", id).put("location", location).put("gib", gib).put("incarnation", incarnation)
    fun validate() {
        require(id.matches(Regex("[a-zA-Z0-9][a-zA-Z0-9_-]{0,10}"))) { "Invalid storage member identity" }
        require(location.matches(Regex("[a-zA-Z0-9][a-zA-Z0-9_-]{0,80}"))) { "Invalid storage location" }
        require(gib in 1..65536) { "Choose a whole disk allocation between 1 and 65536 GiB" }
        require(UUID.fromString(incarnation).toString() == incarnation) { "Invalid owned disk identity" }
    }
    companion object {
        fun from(value: JSONObject) = PhoneStorageDisk(value.getString("id"), value.getString("location"),
            value.getInt("gib"), value.getString("incarnation")).also { it.validate() }
        fun new(location: String, gib: Int) = PhoneStorageDisk("d" + UUID.randomUUID().toString().replace("-", "").take(10),
            location, gib, UUID.randomUUID().toString())
    }
}

data class PhoneStoragePool(val deviceId: String, val poolId: String, val generation: Long, val disks: List<PhoneStorageDisk>) {
    val gib: Int get() = disks.sumOf { it.gib }
    fun validate(owner: String) {
        require(deviceId == owner && UUID.fromString(owner).toString() == owner) { "Storage belongs to another owner; files were preserved" }
        require(UUID.fromString(poolId).toString() == poolId && generation >= 0) { "Invalid storage pool identity" }
        require(disks.size in 1..16 && disks.map { it.id }.distinct().size == disks.size) { "Choose one to sixteen distinct storage disks" }
        disks.forEach { it.validate() }
        require(gib in 15..65536) { "The worker pool requires a total allocation between 15 and 65536 GiB" }
        require(disks.map { it.incarnation }.distinct().size == disks.size) { "Duplicate owned disk identity" }
    }
    fun encode(): String = JSONObject().put("format", 1).put("deviceId", deviceId).put("poolId", poolId)
        .put("generation", generation).put("disks", JSONArray(disks.map { it.json() })).toString()
    fun guestRequest(previous: PhoneStoragePool?): JSONObject = JSONObject().put("format", 1)
        .put("deviceId", deviceId).put("poolId", poolId).put("generation", generation)
        .put("disks", JSONArray(disks.mapIndexed { index, disk -> JSONObject().put("id", disk.id)
            .put("device", "/dev/vd${'b' + index}").put("allocationBytes", disk.bytes)
            .put("initialize", previous?.takeIf { it.poolId == poolId }?.disks?.none { it.id == disk.id } ?: true) }))
    companion object {
        fun decode(text: String, owner: String): PhoneStoragePool {
            require(text.length <= 65536) { "Storage receipt exceeds its supported size" }
            val value = JSONObject(text)
            require(value.getInt("format") == 1)
            val disks = value.getJSONArray("disks")
            require(disks.length() in 1..16)
            return PhoneStoragePool(value.getString("deviceId"), value.getString("poolId"), value.getLong("generation"),
                (0 until disks.length()).map { PhoneStorageDisk.from(disks.getJSONObject(it)) }).also { it.validate(owner) }
        }
    }
}

data class PhoneStorageReview(val target: List<PhoneStorageDisk>, val generation: Long,
    val requiredBytes: Map<String, Long>, val requiresBackup: Boolean)

fun reviewPhoneStorage(current: PhoneStoragePool?, target: List<PhoneStorageDisk>,
                       locations: List<PhoneStorageLocation>): PhoneStorageReview {
    require(target.size in 1..16 && target.map { it.id }.distinct().size == target.size) { "Choose one to sixteen distinct storage disks" }
    target.forEach { it.validate() }
    require(target.sumOf { it.gib.toLong() } in 15L..65536L) { "The worker pool requires 15 to 65536 GiB in total" }
    require(locations.map { it.id }.distinct().size == locations.size) { "Duplicate phone storage location" }
    val catalog = locations.associateBy { it.id }
    val previous = current?.disks.orEmpty().associateBy { it.id }
    // A missing original cannot be silently substituted or reconstructed from a partial pool.
    for (disk in current?.disks.orEmpty() + target) require(catalog[disk.location]?.available == true) {
        "Selected storage is unavailable. Reconnect it before changing the worker pool."
    }
    val backup = previous.values.any { old -> target.none { it.id == old.id && it.gib >= old.gib } }
    val required = mutableMapOf<String, Long>()
    for (disk in target) {
        val old = previous[disk.id]
        if (!backup && old != null && disk.location == old.location && disk.gib == old.gib) continue
        val filesystem = checkNotNull(catalog[disk.location]).filesystem
        required[filesystem] = Math.addExact(required[filesystem] ?: 0, disk.bytes)
    }
    for ((filesystem, bytes) in required) {
        val available = locations.filter { it.filesystem == filesystem && it.available }.minOf { it.freeBytes }
        require(bytes <= (available - 10 * STORAGE_GIB).coerceAtLeast(0)) {
            "Not enough temporary capacity. Keep the existing disks and at least 10 GiB free while verifying their replacements."
        }
    }
    val generation = Math.addExact(current?.generation ?: 0, 1)
    return PhoneStorageReview(target, generation, required, backup)
}

fun storageRecoveryDue(optedIn: Boolean, ownerStopped: Boolean, sharingEnabled: Boolean,
                       replacementAvailable: Boolean, missingSince: Long, now: Long): Boolean =
    optedIn && !ownerStopped && sharingEnabled && replacementAvailable && now >= missingSince && now - missingSince >= 120_000

fun replacementStorage(pool: PhoneStoragePool, locations: List<PhoneStorageLocation>,
                       reclaimedBytes: Map<String, Long>): List<PhoneStorageDisk>? {
    val internal = locations.find { it.id == "internal" && it.available } ?: return null
    val catalog = locations.filter { it.available }.associateBy { it.id }
    val remaining = pool.disks.filter { it.location in catalog }
    if (remaining.sumOf { it.gib.toLong() } !in 15L..65536L) return null
    val free = locations.filter { it.available }.groupBy { it.filesystem }.mapValues { (_, entries) -> entries.minOf { it.freeBytes } }
    val required = mutableMapOf(internal.filesystem to 15 * STORAGE_GIB)
    for (disk in remaining) {
        val filesystem = checkNotNull(catalog[disk.location]).filesystem
        required[filesystem] = (required[filesystem] ?: 0) + disk.bytes
    }
    // Credit only owned allocated blocks that the reviewed whole-pool reset can
    // release. Never choose another location or increase a selected disk's limit.
    if (required.any { (filesystem, bytes) -> bytes + 10 * STORAGE_GIB >
            (free[filesystem] ?: 0) + (reclaimedBytes[filesystem] ?: 0).coerceAtLeast(0) }) return null
    return remaining.map { PhoneStorageDisk.new(it.location, it.gib) }
}
