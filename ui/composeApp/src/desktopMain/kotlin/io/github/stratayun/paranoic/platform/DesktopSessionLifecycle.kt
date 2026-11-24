package io.github.stratayun.paranoic.platform

import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import io.github.stratayun.paranoic.ui.state.AppSession
import java.awt.AWTEvent
import java.awt.EventQueue
import java.awt.Frame
import java.awt.KeyboardFocusManager
import java.awt.Toolkit
import java.awt.Window
import java.awt.event.AWTEventListener
import java.awt.event.WindowEvent
import java.util.concurrent.atomic.AtomicBoolean

/** Connect every owned desktop window to the existing Quick Lock boundary.
 * A modal dialog is part of the application; switching away from that dialog
 * must still lock. Listening only to the main Frame would miss that case.
 * AWT events and Compose desktop effects run on the UI event thread. */
@Composable
fun DesktopSessionLifecycle(window: Window, session: AppSession) {
    DisposableEffect(window, session) {
        val activity = DesktopSessionActivity(window, { it.owner }, session::resume, session::pause)
        val disposed = AtomicBoolean(false)
        val focus = KeyboardFocusManager.getCurrentKeyboardFocusManager()
        val toolkit = Toolkit.getDefaultToolkit()

        fun update(focused: Window?) {
            if (disposed.get()) return
            val minimized = (((window as? Frame)?.extendedState ?: Frame.NORMAL) and Frame.ICONIFIED) != 0
            activity.update(focused, window.isShowing, minimized)
        }
        fun reconcile() = update(focus.focusedWindow)

        val listener = AWTEventListener { event ->
            if (disposed.get() || event !is WindowEvent) return@AWTEventListener
            when (event.id) {
                WindowEvent.WINDOW_GAINED_FOCUS -> update(event.window)
                WindowEvent.WINDOW_LOST_FOCUS -> if (activity.owns(event.window)) {
                    // A known owned dialog stays active. A missing destination
                    // locks immediately; a later snapshot could miss a quick
                    // switch away and back to this application.
                    update(event.oppositeWindow)
                }
                WindowEvent.WINDOW_ICONIFIED -> if (event.window === window) {
                    activity.update(null, window.isShowing, true)
                }
                WindowEvent.WINDOW_STATE_CHANGED -> if (event.window === window) {
                    activity.update(focus.focusedWindow, window.isShowing, (event.newState and Frame.ICONIFIED) != 0)
                }
                WindowEvent.WINDOW_DEICONIFIED,
                WindowEvent.WINDOW_OPENED,
                WindowEvent.WINDOW_CLOSED -> if (event.window === window) reconcile()
                WindowEvent.WINDOW_CLOSING -> if (event.window === window) activity.update(null, false, false)
                else -> Unit
            }
        }
        val mask = AWTEvent.WINDOW_FOCUS_EVENT_MASK or AWTEvent.WINDOW_STATE_EVENT_MASK or AWTEvent.WINDOW_EVENT_MASK
        toolkit.addAWTEventListener(listener, mask)
        // The effect may be installed before the native window is shown.
        EventQueue.invokeLater { reconcile() }
        onDispose {
            disposed.set(true)
            toolkit.removeAWTEventListener(listener)
            activity.dispose()
        }
    }
}

/** Small, headless-testable transition policy. No password/session reopening:
 * resume reloads presentation state only; pause revokes secrets in AppSession.
 * Owner traversal also covers nested Compose and Swing dialogs. */
internal class DesktopSessionActivity<W : Any>(
    private val root: W,
    private val owner: (W) -> W?,
    private val resume: () -> Unit,
    private val pause: () -> Unit,
) {
    private var active: Boolean? = null
    private var disposed = false

    fun owns(candidate: W?): Boolean {
        var current = candidate
        while (current != null) {
            if (current === root) return true
            current = owner(current)
        }
        return false
    }

    fun update(focused: W?, showing: Boolean, minimized: Boolean) {
        if (disposed) return
        val next = showing && !minimized && owns(focused)
        if (next == active) return
        active = next
        if (next) resume() else pause()
    }

    fun dispose() {
        if (disposed) return
        disposed = true
        if (active != false) pause()
        active = false
    }
}
