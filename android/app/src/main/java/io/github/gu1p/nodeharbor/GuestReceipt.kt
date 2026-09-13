package io.github.gu1p.nodeharbor

import org.json.JSONObject
import java.util.UUID

data class GuestReceipt(val deviceId: String, val generation: String, val diskGib: Int, val imageVersion: String, val complete: Boolean) {
    fun encode(): String = JSONObject().put("formatVersion", 1).put("deviceId", deviceId).put("generation", generation)
        .put("diskGib", diskGib).put("imageVersion", imageVersion).put("complete", complete).toString()
    companion object {
        fun decode(text: String, owner: String): GuestReceipt {
            try {
                require(text.length <= 16384)
                val value = JSONObject(text)
                require(value.getInt("formatVersion") == 1 && value.getString("deviceId") == owner && UUID.fromString(owner).toString() == owner)
                val generation = value.getString("generation")
                require(UUID.fromString(generation).toString() == generation)
                val disk = value.getInt("diskGib")
                val version = value.getString("imageVersion")
                require(disk in 15..65536 && version.matches(Regex("[a-zA-Z0-9.-]{1,100}")))
                return GuestReceipt(owner, generation, disk, version, value.getBoolean("complete"))
            } catch (error: Exception) {
                throw IllegalArgumentException("The worker ownership receipt is invalid; its files have been preserved", error)
            }
        }
    }
}

fun canRemoveGuest(receipt: GuestReceipt, owner: String, stopped: Boolean, reset: Boolean): Boolean =
    receipt.deviceId == owner && stopped && reset
