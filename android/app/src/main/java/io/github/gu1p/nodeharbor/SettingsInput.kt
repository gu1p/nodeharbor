package io.github.gu1p.nodeharbor

fun clockMinute(text: String): Int? {
    if (!text.matches(Regex("[0-2][0-9]:[0-5][0-9]"))) return null
    val hour = text.substring(0, 2).toInt()
    return if (hour < 24) hour * 60 + text.substring(3).toInt() else null
}

data class ScheduleDraft(val days: List<Int>, val start: String, val end: String) {
    fun window(): ScheduleWindow? {
        val from = clockMinute(start) ?: return null
        val to = clockMinute(end) ?: return null
        return if (days.isEmpty() || from == to) null else ScheduleWindow(days, from, to)
    }
    companion object {
        fun from(window: ScheduleWindow) = ScheduleDraft(window.days, time(window.startMinute), time(window.endMinute))
        private fun time(minute: Int) = "%02d:%02d".format(java.util.Locale.ROOT, minute / 60, minute % 60)
    }
}
