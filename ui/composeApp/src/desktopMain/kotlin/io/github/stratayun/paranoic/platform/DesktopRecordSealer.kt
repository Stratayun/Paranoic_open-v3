package io.github.stratayun.paranoic.platform

import com.sun.jna.platform.win32.Crypt32Util
import io.github.stratayun.paranoic.nativecore.HostBytes
import io.github.stratayun.paranoic.nativecore.RecordSealer

/** Windows: DPAPI (current user) seals Core journal records. Other desktop
 * systems report unavailable and keep an unsealed journal. */
object DesktopRecordSealer : RecordSealer {
            override fun established() = false
    private val windows = System.getProperty("os.name").startsWith("Windows")
    override fun available() = windows
    override fun seal(data: ByteArray): HostBytes = try {
        if (!windows) HostBytes(2u, byteArrayOf()) else HostBytes(0u, Crypt32Util.cryptProtectData(data))
    } catch (_: Exception) { HostBytes(3u, byteArrayOf()) } finally { data.fill(0) }
    override fun open(data: ByteArray): HostBytes = try {
        if (!windows) HostBytes(2u, byteArrayOf()) else HostBytes(0u, Crypt32Util.cryptUnprotectData(data))
    } catch (_: Exception) { HostBytes(1u, byteArrayOf()) }
}
