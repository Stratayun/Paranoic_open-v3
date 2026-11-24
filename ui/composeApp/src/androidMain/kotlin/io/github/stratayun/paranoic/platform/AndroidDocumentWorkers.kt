package io.github.stratayun.paranoic.platform

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.*
import io.github.stratayun.paranoic.nativecore.DocumentWorkers
import io.github.stratayun.paranoic.nativecore.HostBytes
import io.github.stratayun.paranoic.nativecore.HostWorkerOpened
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

internal object DocumentWorkerProtocol {
    const val START = 1
    const val STOP = 2
    const val READY = 3
}

/** Parent only. Bindings/FDs belong to one worker; there is no same-UID launch. */
class AndroidDocumentWorkers(context: Context) : DocumentWorkers {
    private val context = context.applicationContext
    private val slots = Semaphore(2)
    private val sessions = ConcurrentHashMap<ULong, Session>()
    private val replies = Messenger(Handler(Looper.getMainLooper()) { message ->
        if (message.what == DocumentWorkerProtocol.READY) {
            val state = sessions[message.data.getLong("id").toULong()]
            if (state != null && !state.closed.get()) {
                // Fail closed if a manifest/packaging change lost the isolated UID.
                state.error = if (message.data.getInt("error", 3) == 0
                    && message.data.getInt("uid", Process.myUid()) != Process.myUid()) 0u else 2u
                state.ready.countDown()
            }
        }
        true
    })
    companion object { private val next = AtomicLong(1) }

    private inner class Session(val id: ULong, val input: InputStream, val output: OutputStream) : ServiceConnection {
        val owner = Binder()
        val closed = AtomicBoolean(false)
        val bound = AtomicBoolean(false)
        val connected = CountDownLatch(1)
        val ready = CountDownLatch(1)
        @Volatile var remote: Messenger? = null
        @Volatile var error: UInt = 3u
        override fun onServiceConnected(name: ComponentName, binder: IBinder) {
            if (!closed.get()) remote = Messenger(binder)
            connected.countDown()
        }
        override fun onServiceDisconnected(name: ComponentName) { close() }
        override fun onBindingDied(name: ComponentName) { close() }
        override fun onNullBinding(name: ComponentName) { close() }
        fun unbind() {
            if (bound.compareAndSet(true, false)) try { context.unbindService(this) } catch (_: Exception) {}
        }
        fun close() {
            if (!closed.compareAndSet(false, true)) return
            try { remote?.send(Message.obtain().apply {
                what = DocumentWorkerProtocol.STOP
                data = Bundle().apply { putLong("id", id.toLong()) }
            }) } catch (_: Exception) {}
            try { output.close() } catch (_: Exception) {}
            try { input.close() } catch (_: Exception) {}
            connected.countDown(); ready.countDown()
            sessions.remove(id, this)
            unbind()
            slots.release()
        }
    }

    override fun start(kind: UByte): HostWorkerOpened {
        if (kind.toInt() !in 1..2 || Looper.myLooper() == Looper.getMainLooper()) return HostWorkerOpened(2u, 0uL)
        if (!slots.tryAcquire()) return HostWorkerOpened(4u, 0uL)
        var state: Session? = null
        var toWorker: Array<ParcelFileDescriptor>? = null
        var fromWorker: Array<ParcelFileDescriptor>? = null
        var adopted = false
        try {
            val id = next.getAndIncrement()
            check(id > 0)
            val inputPipe = ParcelFileDescriptor.createReliablePipe().also { toWorker = it }
            val outputPipe = ParcelFileDescriptor.createReliablePipe().also { fromWorker = it }
            val active = Session(id.toULong(), ParcelFileDescriptor.AutoCloseInputStream(outputPipe[0]),
                ParcelFileDescriptor.AutoCloseOutputStream(inputPipe[1]))
            state = active
            sessions[active.id] = active
            val intent = Intent(context, AndroidDocumentWorkerService::class.java)
            check(context.bindService(intent, active, Context.BIND_AUTO_CREATE))
            active.bound.set(true)
            if (active.closed.get()) { active.unbind(); error("Worker binding closed") }
            check(active.connected.await(10, TimeUnit.SECONDS) && !active.closed.get())
            val remote = checkNotNull(active.remote)
            remote.send(Message.obtain().apply {
                what = DocumentWorkerProtocol.START
                replyTo = replies
                data = Bundle().apply {
                    putLong("id", id); putInt("kind", kind.toInt()); putBinder("owner", active.owner)
                    putParcelable("input", inputPipe[0]); putParcelable("output", outputPipe[1])
                }
            })
            check(active.ready.await(10, TimeUnit.SECONDS) && !active.closed.get())
            if (active.error != 0u) return HostWorkerOpened(active.error, 0uL)
            adopted = true
            return HostWorkerOpened(0u, active.id)
        } catch (_: Exception) { return HostWorkerOpened(3u, 0uL) }
        finally {
            // Binder duplicated these ends; this process never keeps worker ends.
            try { toWorker?.get(0)?.close() } catch (_: Exception) {}
            try { fromWorker?.get(1)?.close() } catch (_: Exception) {}
            if (!adopted) {
                if (state != null) state.close() else {
                    try { toWorker?.get(1)?.close() } catch (_: Exception) {}
                    try { fromWorker?.get(0)?.close() } catch (_: Exception) {}
                    slots.release()
                }
            }
        }
    }
    override fun receive(id: ULong, size: UInt): HostBytes = try {
        require(size in 1u..1048576u)
        val state = sessions[id] ?: error("Worker closed")
        check(!state.closed.get())
        val bytes = ByteArray(size.toInt())
        try {
            val count = state.input.read(bytes).coerceAtLeast(0)
            check(!state.closed.get())
            HostBytes(0u, bytes.copyOf(count))
        } finally { bytes.fill(0) }
    } catch (_: Exception) { HostBytes(3u, byteArrayOf()) }

    override fun send(id: ULong, data: ByteArray): UInt = try {
        require(data.size <= 1048576)
        val state = sessions[id] ?: error("Worker closed")
        check(!state.closed.get())
        state.output.write(data); state.output.flush()
        check(!state.closed.get()); 0u
    } catch (_: Exception) { 3u } finally { data.fill(0) }
    override fun stop(id: ULong) { sessions[id]?.close() }
}
