package io.github.gu1p.nodeharbor

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics

@Composable
internal fun WorkloadPanels(items: List<GuestPod>?) {
    for (system in listOf(false, true)) HarborCard {
        Text(if (system) "System components" else "Running workloads",
            style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
        if (system) Text("Health checks and helpers that keep your worker connected.")
        val pods = items?.filter { (it.namespace in setOf("nodeharbor-system", "kube-system")) == system }
        if (pods == null) Text(if (system) "System component inventory is unavailable" else "Workload inventory is unavailable")
        else if (pods.isEmpty()) Text(if (system) "No system components running" else "No workloads running")
        else pods.forEach { pod -> key(pod.uid) {
            val probe = pod.namespace == "nodeharbor-system" && Regex("nodeharbor-probe-[a-z0-9]+").matches(pod.name)
            Text(if (probe) "NodeHarbor health check" else pod.name)
            Text(pod.state)
            if (system) {
                var details by remember { mutableStateOf(false) }
                TextButton(onClick = { details = !details }) { Text(if (details) "Hide component details" else "Component details") }
                if (details) Text("${pod.name} · ${pod.namespace}")
            } else Text(pod.namespace)
        } }
    }
}
