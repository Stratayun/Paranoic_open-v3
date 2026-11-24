package io.github.stratayun.paranoic.platform

import android.app.Service
import android.content.Intent
import android.os.*
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Semaphore
import java.util.concurrent.atomic.AtomicBoolean

/** No Core, Keystore, grants or document URIs in this isolated-UID process.
 * Children inherit its UID, then enter their own seccomp/rlimit sandbox.
 * Only the two pipe descriptors and a fixed decoder kind cross the boundary. */
class AndroidDocumentWorkerService : Service() {
    private val slots = Semaphore(2)
    private val sessions = ConcurrentHashMap<Long, Running>()
    private val messenger = Messenger(Handler(Looper.getMainLooper()) { message ->
        if (message.sendingUid != applicationInfo.uid) return@Handler true
        when (message.what) {
            DocumentWorkerProtocol.START -> start(message)
            DocumentWorkerProtocol.STOP -> sessions[message.data.getLong("id")]?.stop()
        }
        true
    })
    override fun onCreate() {
        super.onCreate()
        check(Process.myUid() != applicationInfo.uid) { "Decoder service must have an isolated UID" }
    }
    override fun onBind(intent: Intent): IBinder = messenger.binder
    override fun onUnbind(intent: Intent): Boolean { sessions.values.forEach { it.stop() }; return false }
    override fun onDestroy() { sessions.values.forEach { it.stop() }; super.onDestroy() }

    private inner class Running(val id: Long, val process: java.lang.Process,
        val input: InputStream, val output: OutputStream, val owner: IBinder) {
        val closed = AtomicBoolean(false)
        val released = AtomicBoolean(false)
        val death = IBinder.DeathRecipient { stop() }
        fun stop() {
            if (!closed.compareAndSet(false, true)) return
            // Close the writer to wake parent reads even if the native child hung.
            try { process.destroyForcibly() } catch (_: Exception) {}
            try { output.close() } catch (_: Exception) {}
            try { input.close() } catch (_: Exception) {}
            try { process.outputStream.close() } catch (_: Exception) {}
            try { process.inputStream.close() } catch (_: Exception) {}
            try { owner.unlinkToDeath(death, 0) } catch (_: Exception) {}
        }
        fun release() {
            if (released.compareAndSet(false, true)) { sessions.remove(id, this); slots.release() }
        }
    }
    private fun reply(message: Message, error: Int) {
        try { message.replyTo?.send(Message.obtain().apply {
            what = DocumentWorkerProtocol.READY
            data = Bundle().apply { putLong("id", message.data.getLong("id")); putInt("error", error); putInt("uid", Process.myUid()) }
        }) } catch (_: Exception) { sessions[message.data.getLong("id")]?.stop() }
    }
    @Suppress("DEPRECATION")
    private fun start(message: Message) {
        val bundle = message.data.apply { classLoader = ParcelFileDescriptor::class.java.classLoader }
        val inputFd = bundle.getParcelable<ParcelFileDescriptor>("input")
        val outputFd = bundle.getParcelable<ParcelFileDescriptor>("output")
        val owner = bundle.getBinder("owner")
        val id = bundle.getLong("id")
        val kind = bundle.getInt("kind")
        if (inputFd == null || outputFd == null || owner == null || id <= 0 || kind !in 1..2
            || sessions.containsKey(id) || !slots.tryAcquire()) {
            try { inputFd?.close() } catch (_: Exception) {}
            try { outputFd?.close() } catch (_: Exception) {}
            reply(message, 2); return
        }
        var running: Running? = null
        try {
            val directory = File(applicationInfo.nativeLibraryDir)
            val worker = File(directory, "libparanoic_pdf_worker.so")
            val args = if (kind == 1) listOf(worker.absolutePath, "--pdfium", File(directory, "libpdfium.so").absolutePath)
                else listOf(worker.absolutePath, "--sevenz")
            // Executable/library paths come only from the signed installation.
            val builder = ProcessBuilder(args).directory(directory).redirectError(File("/dev/null"))
            builder.environment().clear()
            val process = builder.start()
            val active = Running(id, process, ParcelFileDescriptor.AutoCloseInputStream(inputFd),
                ParcelFileDescriptor.AutoCloseOutputStream(outputFd), owner)
            running = active
            sessions[id] = active
            owner.linkToDeath(active.death, 0)
            pump("input-$id", active.input, process.outputStream, active)
            pump("output-$id", process.inputStream, active.output, active)
            Thread({
                try { process.waitFor() } catch (_: InterruptedException) { Thread.currentThread().interrupt() }
                finally { active.stop(); active.release() }
            }, "paranoic-worker-wait-$id").apply { isDaemon = true; start() }
            if (!owner.isBinderAlive) active.stop()
            reply(message, if (active.closed.get()) 3 else 0)
        } catch (_: Exception) {
            if (running != null) { running.stop(); running.release() }
            else {
                try { inputFd.close() } catch (_: Exception) {}
                try { outputFd.close() } catch (_: Exception) {}
                slots.release()
            }
            reply(message, 3)
        }
    }
    private fun pump(name: String, input: InputStream, output: OutputStream, owner: Running) {
        Thread({
            val buffer = ByteArray(64 * 1024)
            try {
                while (!owner.closed.get()) {
                    val count = input.read(buffer)
                    if (count < 0) break
                    output.write(buffer, 0, count); output.flush()
                }
            } catch (_: Exception) {}
            finally { buffer.fill(0); owner.stop() }
        }, "paranoic-worker-$name").apply { isDaemon = true; start() }
    }
}
