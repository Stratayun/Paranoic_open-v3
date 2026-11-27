package io.github.stratayun.paranoic.platform

import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import kotlin.test.*

/** Independent on-disk fixtures, not just encode/decode round trips. */
class GrantCodecMalformedInputTest {
    @Test fun unicodeByteBudgetRejectsOversizeAndPreservesAcceptedReferences() {
        // Each character takes three bytes in writeUTF; character counts alone
        // previously allowed a file larger than the next read would accept.
        val reference = "界".repeat(8192)
        fun grants(count: Int) = List(count) {
            io.github.stratayun.paranoic.nativecore.NativeGrant("grant-$it", reference)
        }
        val accepted = grants(80)
        val encoded = GrantCodec.encode(accepted)
        assertTrue(encoded.size <= GrantCodec.MAX_BYTES)
        val decoded = GrantCodec.decode(encoded)
        assertEquals(accepted.map { it.reference }, decoded.map { it.reference })
        assertFailsWith<IllegalArgumentException> { GrantCodec.encode(grants(128)) }
    }
    private fun bytes(block: DataOutputStream.() -> Unit): ByteArray = ByteArrayOutputStream().also {
        DataOutputStream(it).use(block)
    }.toByteArray()

    @Test fun versionOneKeepsNativeAndServerLocationsAndSkipsRemovedProvider() {
        val input = bytes {
            writeInt(1); writeInt(3)
            writeUTF("local"); writeUTF("/storage/card"); writeBoolean(false)
            writeUTF("obsolete"); writeUTF("content://fixture.documents/tree/root%3Afolder"); writeBoolean(true)
            writeUTF("server"); writeUTF("webdav:sealed"); writeBoolean(false)
        }
        val decoded = GrantCodec.decode(input)
        assertEquals(listOf("local" to "/storage/card", "server" to "webdav:sealed"),
            decoded.map { it.id to it.reference })
        assertEquals(decoded, GrantCodec.decode(GrantCodec.encode(decoded)))
    }

    @Test fun malformedInputsAreRejectedWithoutMutatingEvidence() {
        val valid = bytes { writeInt(1); writeInt(0) }
        val inputs = listOf(
            bytes { writeInt(3); writeInt(0) },
            bytes { writeInt(1); writeInt(-1) },
            bytes { writeInt(1); writeInt(129) },
            bytes { writeInt(1); writeInt(1); writeUTF("id") },
            bytes { writeInt(1); writeInt(1); writeUTF("id"); writeUTF("/storage/card") },
            bytes { writeInt(2); writeInt(1); writeUTF("id") },
            valid + byteArrayOf(1), ByteArray(2 * 1024 * 1024 + 1),
        ) + (0 until valid.size).map { valid.copyOf(it) }
        inputs.forEachIndexed { index, input ->
            val before = input.copyOf()
            assertFails("fixture $index unexpectedly accepted") { GrantCodec.decode(input) }
            assertContentEquals(before, input, "decoder overwrote evidence $index")
        }
    }
}

