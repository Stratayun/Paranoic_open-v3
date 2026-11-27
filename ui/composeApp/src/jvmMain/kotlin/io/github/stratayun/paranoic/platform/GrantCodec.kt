package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.nativecore.NativeGrant
import java.io.*

object GrantCodec {
    private const val VERSION = 2
    const val MAX_BYTES = 2 * 1024 * 1024
    fun encode(grants: List<NativeGrant>): ByteArray {
        require(grants.size <= 128)
        val output = ByteArrayOutputStream()
        DataOutputStream(output).use { writer ->
            writer.writeInt(VERSION); writer.writeInt(grants.size)
            grants.forEach {
                require(it.reference.length <= 8192)
                writer.writeUTF(it.id); writer.writeUTF(it.reference)
                require(output.size() <= MAX_BYTES) { "Grant store exceeds byte limit" }
            }
        }
        return output.toByteArray()
    }
    fun decode(data: ByteArray): List<NativeGrant> {
        require(data.size <= MAX_BYTES)
        return DataInputStream(ByteArrayInputStream(data)).use { reader ->
            val version = reader.readInt()
            require(version == 1 || version == VERSION)
            val count = reader.readInt(); require(count in 0..128)
            val grants = List(count) {
                val id = reader.readUTF()
                val reference = reader.readUTF()
                require(reference.length <= 8192)
                // In v1, the trailing flag marks discontinued provider-backed locations.
                if (version == 1 && reader.readBoolean()) null else NativeGrant(id, reference)
            }
            require(reader.available() == 0)
            grants.filterNotNull()
        }
    }
}


