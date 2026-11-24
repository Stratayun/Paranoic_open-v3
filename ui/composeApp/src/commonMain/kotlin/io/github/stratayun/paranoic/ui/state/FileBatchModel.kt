package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*

const val MAX_FILE_SELECTION = 128
enum class FileBatchKind { Copy, Move, Delete }

/** A confirmed snapshot, never a reference to the live selection. No passwords. */
data class FileBatchPlan(val kind: FileBatchKind, val sources: List<FileItem>, val parent: Location? = null,
    val conflict: NameConflict = NameConflict.Fail, val recursive: Boolean = false) {
    init {
        require(sources.size in 1..MAX_FILE_SELECTION)
        require(sources.map { it.location }.distinct().size == sources.size)
        require(sources.map { it.name }.distinct().size == sources.size)
        require(sources.all { it.kind == EntryKind.File || it.kind == EntryKind.Directory })
        require((kind == FileBatchKind.Delete) == (parent == null))
        require(!recursive || kind == FileBatchKind.Delete)
        require(conflict != NameConflict.Replace)
        require(kind == FileBatchKind.Copy || conflict == NameConflict.Fail)
        val first = sources.first().location
        require(sources.all { it.location.storageId == first.storageId &&
            it.location.path.substringBeforeLast('/', "") == first.path.substringBeforeLast('/', "") })
    }
    fun allowed(spaces: List<Space>): Boolean = sources.all { source ->
        plausibleOperationName(source.name) && when (kind) {
            FileBatchKind.Delete -> source.canDelete(spaces)
            FileBatchKind.Copy, FileBatchKind.Move -> {
                val destination = parent ?: return@all false
                val mode = if (kind == FileBatchKind.Copy) TransferMode.Copy else TransferMode.Move
                source.canTransferTo(destination, mode, spaces) &&
                    (!source.isTarget(destination, source.name) || (kind == FileBatchKind.Copy && conflict == NameConflict.KeepBoth))
            }
        }
    }
}

enum class BatchDelivery { Pending, Sending, Accepted, Rejected, Unknown, NotSent }
data class FileBatchEntry(val requestId: String, val source: FileItem?, val delivery: BatchDelivery = BatchDelivery.Pending,
    val jobId: String? = null, val fault: CoreFault? = null)

/** Transient grouping only. Each accepted job belongs to the durable Rust journal. */
data class FileBatchReport(val kind: FileBatchKind, val entries: List<FileBatchEntry>, val sending: Boolean = true,
    val stopRequested: Boolean = false) {
    init {
        require(entries.size in 1..MAX_FILE_SELECTION)
        require(entries.map { it.requestId }.distinct().size == entries.size)
        require(entries.all { it.requestId.isNotBlank() })
    }
    fun interrupted() = copy(sending = false, stopRequested = true, entries = entries.map {
        when (it.delivery) {
            BatchDelivery.Sending -> it.copy(delivery = BatchDelivery.Unknown, fault = CoreFault.OutcomeUnknown)
            BatchDelivery.Pending -> it.copy(delivery = BatchDelivery.NotSent)
            else -> it
        }
    })
    fun redact() = copy(entries = entries.map { it.copy(source = null) })
    /** Journal reads can resolve a lost submission reply; absence never triggers a retry. */
    fun reconcile(jobs: List<JobItem>): FileBatchReport = copy(entries = entries.map { entry ->
        val job = jobs.firstOrNull { it.id == entry.jobId || it.id == entry.requestId }
        if (entry.delivery == BatchDelivery.Unknown && job != null)
            entry.copy(delivery = BatchDelivery.Accepted, jobId = job.id, fault = null) else entry
    })
}

enum class FileBatchStatus { Sending, Queued, Running, Done, Skipped, Failed, Cancelled, NeedsAttention, Unknown, NotSent }
fun FileBatchEntry.status(jobs: List<JobItem>): FileBatchStatus = when (delivery) {
    BatchDelivery.Pending, BatchDelivery.Sending -> FileBatchStatus.Sending
    BatchDelivery.Rejected -> FileBatchStatus.Failed
    BatchDelivery.Unknown -> FileBatchStatus.Unknown
    BatchDelivery.NotSent -> FileBatchStatus.NotSent
    BatchDelivery.Accepted -> when (val job = jobs.firstOrNull { it.id == jobId }) {
        null -> FileBatchStatus.Unknown
        else -> when (job.phase) {
            JobPhase.Queued -> FileBatchStatus.Queued
            JobPhase.Running -> FileBatchStatus.Running
            JobPhase.Succeeded -> when (job.outcome) {
                JobOutcome.Done -> FileBatchStatus.Done
                JobOutcome.Skipped -> FileBatchStatus.Skipped
                JobOutcome.Unknown -> FileBatchStatus.Unknown
            }
            JobPhase.Failed -> FileBatchStatus.Failed
            JobPhase.Cancelled -> FileBatchStatus.Cancelled
            JobPhase.NeedsAttention -> FileBatchStatus.NeedsAttention
        }
    }
}

data class FileBatchSummary(val total: Int, val done: Int, val skipped: Int, val failed: Int,
    val cancelled: Int, val attention: Int, val unknown: Int, val notSent: Int) {
    val settled get() = done + skipped + failed + cancelled + attention + notSent
}
fun FileBatchReport.summary(jobs: List<JobItem>): FileBatchSummary {
    val counts = entries.groupingBy { it.status(jobs) }.eachCount()
    fun count(status: FileBatchStatus) = counts[status] ?: 0
    return FileBatchSummary(entries.size, count(FileBatchStatus.Done), count(FileBatchStatus.Skipped),
        count(FileBatchStatus.Failed), count(FileBatchStatus.Cancelled), count(FileBatchStatus.NeedsAttention),
        count(FileBatchStatus.Unknown), count(FileBatchStatus.NotSent))
}
