package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

/** Submits independent jobs to Core, without filesystem IO, retries or a second queue.
 * Stops after an uncertain reply. Already accepted jobs are never rolled back. */
class FileBatchDispatcher(private val core: CoreGateway) {
    suspend fun send(plan: FileBatchPlan, requestIds: List<String>, shouldStop: () -> Boolean,
        update: (FileBatchReport) -> Unit, accepted: (JobItem) -> Unit) {
        val sources = plan.sources.toList()
        val ids = requestIds.toList()
        require(ids.size == sources.size)
        var report = FileBatchReport(plan.kind, sources.mapIndexed { i, source -> FileBatchEntry(ids[i], source) })
        update(report)
        try {
            for (index in sources.indices) {
                currentCoroutineContext().ensureActive()
                if (shouldStop()) break
                report = report.copy(entries = report.entries.mapIndexed { i, entry ->
                    if (i == index) entry.copy(delivery = BatchDelivery.Sending) else entry
                })
                update(report)
                val source = sources[index]
                val result = try {
                    when (plan.kind) {
                        FileBatchKind.Delete -> core.delete(source.location, plan.recursive && source.kind == EntryKind.Directory, ids[index])
                        FileBatchKind.Copy, FileBatchKind.Move -> core.transfer(source.location, requireNotNull(plan.parent), source.name,
                            if (plan.kind == FileBatchKind.Copy) TransferMode.Copy else TransferMode.Move, plan.conflict, ids[index])
                    }
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { CoreResult.Failure(CoreFault.OutcomeUnknown) }
                currentCoroutineContext().ensureActive()
                report = report.copy(entries = report.entries.mapIndexed { i, entry ->
                    if (i != index) entry else when (result) {
                        is CoreResult.Value -> entry.copy(delivery = BatchDelivery.Accepted, jobId = result.value.id)
                        is CoreResult.Failure -> entry.copy(delivery = if (result.fault == CoreFault.OutcomeUnknown)
                            BatchDelivery.Unknown else BatchDelivery.Rejected, fault = result.fault)
                    }
                })
                update(report)
                if (result is CoreResult.Value) accepted(result.value)
                if (result is CoreResult.Failure && result.fault == CoreFault.OutcomeUnknown) break
            }
            report = report.copy(sending = false, stopRequested = shouldStop(), entries = report.entries.map {
                if (it.delivery == BatchDelivery.Pending) it.copy(delivery = BatchDelivery.NotSent) else it
            })
            update(report)
        } catch (cancelled: CancellationException) {
            update(report.interrupted())
            throw cancelled
        }
    }
}
