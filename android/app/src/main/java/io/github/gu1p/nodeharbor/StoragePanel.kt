package io.github.gu1p.nodeharbor

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp

data class StorageUiState(val locations: List<PhoneStorageLocation> = emptyList(), val pool: PhoneStoragePool? = null,
    val review: PhoneStorageReview? = null, val pending: Boolean = false, val retained: Int = 0,
    val automaticRecovery: Boolean = false, val busy: Boolean = false, val missing: Boolean = false,
    val message: String = "Worker storage is private to this app. Available app storage locations appear after connecting to your fleet.",
    val excluded: List<PhoneStorageDisk> = emptyList())

@Composable
fun StoragePanel(state: StorageUiState, enrolled: Boolean, onAction: (UiAction) -> Unit) {
    var editing by remember { mutableStateOf(false) }
    var draft by remember { mutableStateOf(emptyList<PhoneStorageDisk>()) }
    var confirmation by remember { mutableStateOf("") }
    HarborCard {
        Text("Storage locations", style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
        Text(state.message, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        state.locations.forEach { location ->
            Text("${location.label} · ${if (location.available) "${location.freeBytes / STORAGE_GIB} GiB free" else "Unavailable"}")
        }
        state.pool?.let { pool ->
            Text("${pool.gib} GiB across ${pool.disks.size} disk(s)")
            pool.disks.forEachIndexed { index, disk ->
                Text("Disk ${index + 1}: ${state.locations.find { it.id == disk.location }?.label ?: "Unavailable location"} · ${disk.gib} GiB")
            }
        }
        Text("Locations on the same physical filesystem share their free-space budget. Removing a selected volume stops the whole worker.",
            style = MaterialTheme.typography.bodySmall)
        OutlinedButton(onClick = {
            draft = state.pool?.disks ?: listOf(PhoneStorageDisk.new(state.locations.firstOrNull { it.available }?.id ?: "internal", 15))
            editing = true
        }, enabled = enrolled && !state.busy && !state.pending && !state.missing) { Text("Edit storage locations") }
        if (state.pending) OutlinedButton(onClick = { onAction(UiAction.Storage("resume")) }, enabled = enrolled && !state.busy) { Text("Resume storage change") }
        if (state.busy) OutlinedButton(onClick = { onAction(UiAction.Storage("cancel")) }) { Text("Stop storage work") }
        if (state.retained > 0) {
            Text("${state.retained} original disk copy/copies retained. They are excluded from the active pool.")
            OutlinedButton(onClick = { confirmation = "cleanup" }, enabled = !state.busy && !state.pending) { Text("Remove retained copies") }
        }
        state.excluded.forEach { disk ->
            val location = state.locations.find { it.id == disk.location }
            Text("Previous storage on ${location?.label ?: "an unavailable volume"} stays excluded from the active worker.")
            OutlinedButton(onClick = {
                onAction(UiAction.ReviewStorage(state.pool!!.disks + PhoneStorageDisk.new(disk.location, disk.gib)))
            }, enabled = enrolled && state.pool != null && !state.busy && !state.pending && location?.available == true) {
                Text("Restore ${disk.gib} GiB on ${location?.label ?: "returned storage"}")
            }
        }
        Row(Modifier.fillMaxWidth()) {
            Text("Automatically replace lost storage", Modifier.weight(1f).padding(top = 12.dp))
            Switch(checked = state.automaticRecovery, enabled = enrolled && !state.busy,
                onCheckedChange = { if (it) confirmation = "recovery" else onAction(UiAction.StorageRecovery(false)) },
                modifier = Modifier.semantics { contentDescription = "Automatically replace lost storage" })
        }
        Text("Off by default. After two minutes of missing storage, this can discard the entire worker pool and rebuild within the limits of remaining selected disks. Pause and Stop take priority.",
            style = MaterialTheme.typography.bodySmall)
        OutlinedButton(onClick = { confirmation = "delete" }, enabled = enrolled && !state.busy) { Text("Delete all worker storage") }
    }
    if (editing) AlertDialog(onDismissRequest = { editing = false }, title = { Text("Edit storage locations") }, text = {
        Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Choose app-specific volumes and allocation. Review temporary capacity before changing any data.")
            draft.forEachIndexed { index, disk ->
                var menu by remember(disk.id) { mutableStateOf(false) }
                var amount by remember(disk.id) { mutableStateOf(disk.gib.toString()) }
                fun update(value: PhoneStorageDisk) { draft = draft.toMutableList().also { it[index] = value } }
                Box {
                    OutlinedButton(onClick = { menu = true }) {
                        Text("Disk ${index + 1} location: ${state.locations.find { it.id == disk.location }?.label ?: "Unavailable"}")
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        state.locations.forEach { location -> DropdownMenuItem(text = { Text(location.label) }, enabled = location.available,
                            onClick = { update(disk.copy(location = location.id)); menu = false }) }
                    }
                }
                OutlinedTextField(value = amount, onValueChange = { amount = it; update(disk.copy(gib = it.toIntOrNull() ?: 0)) },
                    label = { Text("Disk ${index + 1} allocation in GiB") }, singleLine = true, isError = disk.gib !in 1..65536,
                    modifier = Modifier.fillMaxWidth())
                if (draft.size > 1) TextButton(onClick = { draft = draft.filter { it.id != disk.id } }) { Text("Remove disk ${index + 1}") }
            }
            OutlinedButton(onClick = { draft = draft + PhoneStorageDisk.new(state.locations.firstOrNull { it.available }?.id ?: "internal", 15) },
                enabled = draft.size < 16) { Text("Add storage disk") }
        }
    }, confirmButton = { TextButton(onClick = { editing = false; onAction(UiAction.ReviewStorage(draft)) },
        enabled = draft.isNotEmpty() && draft.all { it.gib in 1..65536 }) { Text("Review storage changes") } },
        dismissButton = { TextButton(onClick = { editing = false }) { Text("Cancel") } })
    state.review?.let { review -> AlertDialog(onDismissRequest = { onAction(UiAction.Storage("dismiss-review")) },
        title = { Text("Review storage changes") }, text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("${review.target.sumOf { it.gib }} GiB across ${review.target.size} disk(s)")
                review.target.forEachIndexed { index, disk -> Text("Disk ${index + 1}: ${state.locations.find { it.id == disk.location }?.label ?: "Unavailable"} · ${disk.gib} GiB") }
                Text("Temporary disk allocation: ${review.requiredBytes.values.sum() / STORAGE_GIB} GiB. Existing disk copies remain until you remove them after verification.")
                if (review.requiresBackup) Text("Shrinking or removing disks requires a verified whole-pool backup on the guest system disk. The change stops safely if that private backup space is insufficient.")
                Text("Current jobs drain before storage maintenance and may be interrupted at your drain deadline. Enrollment and sharing preferences are preserved.")
            }
        }, confirmButton = { TextButton(onClick = { onAction(UiAction.Storage("apply")) }) { Text("Apply storage changes") } },
        dismissButton = { TextButton(onClick = { onAction(UiAction.Storage("dismiss-review")) }) { Text("Cancel") } }) }
    if (confirmation.isNotEmpty()) AlertDialog(onDismissRequest = { confirmation = "" },
        title = { Text(when (confirmation) { "recovery" -> "Allow whole-pool data loss?"; "cleanup" -> "Remove retained copies?"; else -> "Delete all worker storage?" }) },
        text = { Text(when (confirmation) {
            "recovery" -> "After two minutes, all worker data may be discarded and rebuilt within the allocations of remaining selected disks. A returned old disk stays excluded. An explicit Pause or Stop prevents automatic recovery."
            "cleanup" -> "After confirmed worker shutdown, permanently remove original disk copies retained by completed storage changes."
            else -> "Drain work, remove the worker’s fleet access, confirm Android process death, and delete owned worker disks. Enrollment is retained. Storage and sharing stay disabled."
        }) }, confirmButton = { TextButton(onClick = {
            val selected = confirmation; confirmation = ""
            if (selected == "recovery") onAction(UiAction.StorageRecovery(true)) else onAction(UiAction.Storage(selected))
        }) { Text(when (confirmation) { "recovery" -> "Allow automatic data loss"; "cleanup" -> "Remove retained copies"; else -> "Delete worker storage" }) } },
        dismissButton = { TextButton(onClick = { confirmation = "" }) { Text("Cancel") } })
}
