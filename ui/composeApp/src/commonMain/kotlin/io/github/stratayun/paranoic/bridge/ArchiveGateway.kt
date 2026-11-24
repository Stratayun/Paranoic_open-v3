package io.github.stratayun.paranoic.bridge

/** Request-bound preparation and publication. No password is persisted in a request. */
interface ArchiveGateway {
    suspend fun archiveListWithRequest(source: Location, format: ArchiveFormat, password: String?, requestId: String): CoreResult<List<ArchiveEntry>>
    suspend fun archiveExtractWithRequest(source: Location, format: ArchiveFormat, password: String?, parent: Location, name: String, requestId: String): CoreResult<Unit>
    suspend fun archiveCreateWithRequest(sources: List<Location>, format: ArchiveFormat, password: String?, parent: Location, name: String, requestId: String): CoreResult<Unit>
    suspend fun cancelArchiveRequest(requestId: String): CoreResult<Unit>
}
// Existing gateways/test doubles keep their established archive contract.
suspend fun CoreGateway.archiveListRequested(source: Location, format: ArchiveFormat, password: String?, id: String) =
    if (this is ArchiveGateway) archiveListWithRequest(source, format, password, id) else archiveList(source, format, password)
suspend fun CoreGateway.archiveExtractRequested(source: Location, format: ArchiveFormat, password: String?, parent: Location, name: String, id: String) =
    if (this is ArchiveGateway) archiveExtractWithRequest(source, format, password, parent, name, id) else archiveExtract(source, format, password, parent, name)
suspend fun CoreGateway.archiveCreateRequested(sources: List<Location>, format: ArchiveFormat, password: String?, parent: Location, name: String, id: String) =
    if (this is ArchiveGateway) archiveCreateWithRequest(sources, format, password, parent, name, id) else archiveCreate(sources, format, password, parent, name)
suspend fun CoreGateway.cancelArchiveRequested(id: String): CoreResult<Unit> =
    if (this is ArchiveGateway) cancelArchiveRequest(id) else CoreResult.Failure(CoreFault.Unsupported)
