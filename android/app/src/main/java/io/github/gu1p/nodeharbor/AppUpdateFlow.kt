package io.github.gu1p.nodeharbor

interface AppUpdateRuntime {
    fun active(): Boolean
    fun downloadAndVerify()
    fun beginMaintenance()
    fun readiness(): UpdateGate
    fun awaitChange()
    fun install()
    fun cancelMaintenance()
}

fun applyAppUpdate(runtime: AppUpdateRuntime): Boolean {
    if (!runtime.active()) return false
    runtime.downloadAndVerify()
    if (!runtime.active()) return false
    var maintenance = true
    try {
        runtime.beginMaintenance()
        while (runtime.active()) {
            if (runtime.readiness() == UpdateGate.Ready) break
            runtime.awaitChange()
        }
        if (!runtime.active()) return false
        runtime.install()
        maintenance = false // Android now owns installation and its result callback.
        return true
    } finally { if (maintenance) runtime.cancelMaintenance() }
}
