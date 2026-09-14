package io.github.gu1p.nodeharbor

enum class UpdateGate { Preparing, WaitingForController, WaitingForJobs, StopWorker, WaitingForTermination, Ready }

/** Update maintenance has no job-eviction deadline. Emergency owner stopping remains independent. */
fun updateGate(hasVm: Boolean, preparing: Boolean, confirmedIdle: Boolean, controllerFresh: Boolean,
               localWorkloads: Int?, controllerWorkloads: Int?): UpdateGate = when {
    preparing -> UpdateGate.Preparing
    !hasVm -> if (confirmedIdle) UpdateGate.Ready else UpdateGate.WaitingForTermination
    !controllerFresh -> UpdateGate.WaitingForController
    localWorkloads != 0 || controllerWorkloads != 0 -> UpdateGate.WaitingForJobs
    else -> UpdateGate.StopWorker
}
