package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.nativecore.NativeGrant
import java.io.*

object GrantCodec {
    const val MAX_BYTES = 2 * 1024 * 1024
    fun encode(grants: List<NativeGrant>): ByteArray {
        require(grants.size <= 128)
        val output = ByteArrayOutputStream()
        DataOutputStream(output).use { writer ->
            writer.writeInt(1); writer.writeInt(grants.size)
            grants.forEach {
                require(it.reference.length <= 8192)
                writer.writeUTF(it.id); writer.writeUTF(it.reference); writer.writeBoolean(it.androidTree)
                require(output.size() <= MAX_BYTES) { "Grant store exceeds byte limit" }
            }
        }
        return output.toByteArray()
    }
    fun decode(data: ByteArray): List<NativeGrant> {
        require(data.size <= MAX_BYTES)
        return DataInputStream(ByteArrayInputStream(data)).use { reader ->
            require(reader.readInt() == 1)
            val count = reader.readInt(); require(count in 0..128)
            val grants = List(count) {
                val id = reader.readUTF()
                val reference = reader.readUTF()
                require(reference.length <= 8192)
                NativeGrant(id, reference, reader.readBoolean())
            }
            require(reader.available() == 0)
            grants
        }
    }
}


