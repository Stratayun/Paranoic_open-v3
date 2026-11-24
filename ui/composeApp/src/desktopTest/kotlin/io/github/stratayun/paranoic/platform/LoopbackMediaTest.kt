package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.bridge.MediaSource
import java.net.Socket
import java.net.URI
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*

class LoopbackMediaTest {
    private class Source : MediaSource {
        val reads = AtomicInteger()
        val closes = AtomicInteger()
        private val data = "0123456789".toByteArray()
        override val length = data.size.toLong()
        override fun readAt(offset: Long, size: Int): ByteArray {
            reads.incrementAndGet()
            require(size in 0..65536)
            return data.copyOfRange(offset.toInt(), minOf(data.size, offset.toInt() + size))
        }
        override fun close() { closes.incrementAndGet() }
    }
    private data class Response(val status: Int, val headers: Map<String, String>, val body: String)
    /** Raw loopback HTTP exercises Host/Origin without JVM header rewriting. */
    private fun request(uri: URI, method: String = "GET", path: String = uri.rawPath,
        host: String = "127.0.0.1:" + uri.port, headers: List<String> = emptyList()): Response {
        val text = Socket().use { socket ->
            socket.connect(java.net.InetSocketAddress("127.0.0.1", uri.port), 2000)
            socket.soTimeout = 2000
            socket.getOutputStream().write((listOf("$method $path HTTP/1.1", "Host: $host", "Connection: close") +
                headers + listOf("", "")).joinToString("\r\n").toByteArray(Charsets.US_ASCII))
            socket.getInputStream().bufferedReader(Charsets.ISO_8859_1).readText()
        }
        val split = text.indexOf("\r\n\r\n")
        check(split >= 0) { "incomplete HTTP response" }
        val lines = text.substring(0, split).split("\r\n")
        return Response(lines.first().split(' ')[1].toInt(), lines.drop(1).associate {
            it.substringBefore(':').lowercase() to it.substringAfter(':').trim()
        }, text.substring(split + 4))
    }

    @Test fun rangesAndHeadExposeOnlyRequestedBytes() {
        val source = Source()
        LoopbackMedia(source, "audio/test").use { media ->
            val range = request(media.uri, headers = listOf("Range: bytes=2-4"))
            assertEquals(206, range.status)
            assertEquals("234", range.body)
            assertEquals("bytes 2-4/10", range.headers["content-range"])
            assertEquals("no-store", range.headers["cache-control"])
            assertEquals("nosniff", range.headers["x-content-type-options"])
            assertEquals("89", request(media.uri, headers = listOf("Range: bytes=-2")).body)
            val before = source.reads.get()
            val head = request(media.uri, method = "HEAD")
            assertEquals(200, head.status)
            assertEquals("10", head.headers["content-length"])
            assertEquals("", head.body)
            assertEquals(before, source.reads.get())
        }
        assertEquals(1, source.closes.get())
    }

    @Test fun invalidRangeAndUntrustedRequestsNeverReadSource() {
        val source = Source()
        LoopbackMedia(source, "audio/test").use { media ->
            for (range in listOf("bytes=10-", "bytes=5-2", "bytes=-0", "bytes=0-1,3-4", "bytes=999999999999999999999-")) {
                val response = request(media.uri, headers = listOf("Range: $range"))
                assertEquals(416, response.status, range)
                assertEquals("bytes */10", response.headers["content-range"])
            }
            assertEquals(403, request(media.uri, host = "untrusted.invalid").status)
            assertEquals(403, request(media.uri, headers = listOf("Origin: https://untrusted.invalid")).status)
            assertEquals(403, request(media.uri, path = media.uri.rawPath + "?query=1").status)
            assertEquals(403, request(media.uri, path = media.uri.rawPath + "/extra").status)
            assertEquals(405, request(media.uri, method = "POST").status)
            assertEquals(0, source.reads.get())
        }
    }

    @Test fun closeReleasesTheCoreReaderExactlyOnce() {
        val source = Source()
        val media = LoopbackMedia(source, "audio/test")
        try { media.close(); media.close(); assertEquals(1, source.closes.get()) }
        finally { media.close() }
    }
}
