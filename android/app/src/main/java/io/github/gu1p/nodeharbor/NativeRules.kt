package io.github.gu1p.nodeharbor

import org.json.JSONObject

object NativeRules {
    init { System.loadLibrary("nodeharbor") }
    private external fun nativeRequest(input: ByteArray): ByteArray
    fun request(input: String): String = nativeRequest(input.toByteArray(Charsets.UTF_8)).toString(Charsets.UTF_8)
    fun evaluate(policy: PhonePolicy, observation: JSONObject): PhoneDecision {
        val result = JSONObject(request(JSONObject().put("operation", "evaluate")
            .put("policy", policy.sharedJson()).put("observation", observation).toString()))
        return PhoneDecision(result.getBoolean("allowed"), result.getString("reason"))
    }
    fun validate(policy: PhonePolicy, host: JSONObject): String? {
        val response = request(JSONObject().put("operation", "validate").put("policy", policy.sharedJson()).put("host", host).toString())
        return if (response == "null") null else org.json.JSONArray("[$response]").getString(0)
    }
    fun transition(permitted: Boolean, running: Boolean, drainingSince: Long?, now: Long, drainSeconds: Int, workloads: Int): String {
        val input = JSONObject().put("permitted", permitted).put("running", running)
            .put("draining_since", drainingSince ?: JSONObject.NULL).put("now", now).put("drain_seconds", drainSeconds).put("workloads", workloads)
        return org.json.JSONArray("[${request(JSONObject().put("operation", "transition").put("input", input).toString())}]").getString(0)
    }
}
