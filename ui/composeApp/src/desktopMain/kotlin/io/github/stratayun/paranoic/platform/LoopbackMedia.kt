package io.github.stratayun.paranoic.platform

import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import io.github.stratayun.paranoic.bridge.MediaSource
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.URI
import java.security.SecureRandom
import java.util.Base64
import java.util.concurrent.*
import java.util.concurrent.atomic.AtomicBoolean

/** JavaFX requires a URI. A private loopback range endpoint streams one Core
 * reader; no file path, external bind, disk cache, remote URL or server logging.
 * This adapter is for ordinary Files only, not a Private Vault transport.
 */
class LoopbackMedia(private val source: MediaSource, private val mime: String) : AutoCloseable {
    private val closed = AtomicBoolean(false)
    private val token = ByteArray(32).also { SecureRandom().nextBytes(it) }
        .let { Base64.getUrlEncoder().withoutPadding().encodeToString(it) }
    private val workers = ThreadPoolExecutor(2, 2, 0, TimeUnit.SECONDS, ArrayBlockingQueue(16),
        ThreadFactory { runnable -> Thread(runnable, "paranoic-media").apply { isDaemon = true } },
        ThreadPoolExecutor.AbortPolicy())
    private val server = HttpServer.create(InetSocketAddress(InetAddress.getByName("127.0.0.1"), 0), 4)
    val uri: URI
    init {
        server.executor = workers
        server.createContext("/$token") { exchange -> serve(exchange) }
        server.start()
        uri = URI("http://127.0.0.1:${server.address.port}/$token")
    }
    private fun serve(exchange: HttpExchange) {
        try {
            val expectedHost = "127.0.0.1:${server.address.port}"
            if (closed.get() || exchange.requestURI.rawPath != "/$token" || exchange.requestURI.rawQuery != null ||
                exchange.requestHeaders.getFirst("Host") != expectedHost ||
                exchange.requestHeaders.getFirst("Origin") != null) {
                exchange.sendResponseHeaders(403, -1); return
            }
            if (exchange.requestMethod !in listOf("GET", "HEAD")) { exchange.sendResponseHeaders(405, -1); return }
            val length = source.length
            var start = 0L
            var end = length - 1
            val range = exchange.requestHeaders.getFirst("Range")
            if (range != null) {
                val match = Regex("bytes=([0-9]*)-([0-9]*)").matchEntire(range)
                if (match == null || length == 0L) { invalidRange(exchange, length); return }
                val a = match.groupValues[1]; val b = match.groupValues[2]
                if (a.isEmpty()) {
                    val suffix = b.toLongOrNull()
                    if (suffix == null || suffix <= 0) { invalidRange(exchange, length); return }
                    start = (length - suffix).coerceAtLeast(0)
                } else {
                    start = a.toLongOrNull() ?: -1
                    if (b.isNotEmpty()) end = b.toLongOrNull()?.coerceAtMost(length - 1) ?: -1
                }
                if (start < 0 || start >= length || end < start) { invalidRange(exchange, length); return }
            }
            val count = (end - start + 1).coerceAtLeast(0)
            exchange.responseHeaders.set("Content-Type", mime)
            exchange.responseHeaders.set("Accept-Ranges", "bytes")
            exchange.responseHeaders.set("Cache-Control", "no-store")
            exchange.responseHeaders.set("X-Content-Type-Options", "nosniff")
            exchange.responseHeaders.set("Content-Length", count.toString())
            if (range != null) exchange.responseHeaders.set("Content-Range", "bytes $start-$end/$length")
            exchange.sendResponseHeaders(if (range == null) 200 else 206,
                if (exchange.requestMethod == "HEAD" || count == 0L) -1 else count)
            if (exchange.requestMethod == "GET" && count > 0) exchange.responseBody.use { output ->
                var offset = start
                var remaining = count
                while (!closed.get() && remaining > 0) {
                    val data = source.readAt(offset, minOf(65536L, remaining).toInt())
                    if (data.isEmpty()) throw java.io.IOException("media truncated")
                    // Audit A48: the decrypted chunk is wiped even when the socket write fails.
                    try { output.write(data) } finally { data.fill(0) }
                    offset += data.size; remaining -= data.size
                }
            }
        } catch (_: Exception) {
            // Decoder receives an incomplete/failed stream, never a fabricated success.
        } finally { exchange.close() }
    }
    private fun invalidRange(exchange: HttpExchange, length: Long) {
        exchange.responseHeaders.set("Content-Range", "bytes */$length")
        exchange.sendResponseHeaders(416, -1)
    }
    override fun close() {
        if (closed.compareAndSet(false, true)) {
            server.stop(0); workers.shutdownNow(); source.close()
        }
    }
}

