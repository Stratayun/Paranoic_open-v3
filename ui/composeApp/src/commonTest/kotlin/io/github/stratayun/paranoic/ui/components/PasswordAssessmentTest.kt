package io.github.stratayun.paranoic.ui.components

import androidx.compose.runtime.*
import androidx.compose.runtime.snapshots.Snapshot
import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PasswordAssessmentTest {
    @Test
    fun changingTheInputWithdrawsThePreviousVerdictUntilCoreReplies() = runTest {
        val requests = mutableMapOf<String, CompletableDeferred<CoreResult<PasswordFeedback>>>()
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override val connected = true
            override suspend fun passwordFeedback(password: String): CoreResult<PasswordFeedback> =
                requests.getOrPut(password) { CompletableDeferred() }.await()
        }
        val clock = BroadcastFrameClock()
        val recomposer = Recomposer(coroutineContext + clock)
        val job = backgroundScope.launch(UnconfinedTestDispatcher(testScheduler) + clock) {
            recomposer.runRecomposeAndApplyChanges()
        }
        val composition = Composition(object : AbstractApplier<Unit>(Unit) {
            override fun insertTopDown(index: Int, instance: Unit) = Unit
            override fun insertBottomUp(index: Int, instance: Unit) = Unit
            override fun remove(index: Int, count: Int) = Unit
            override fun move(from: Int, to: Int, count: Int) = Unit
            override fun onClear() = Unit
        }, recomposer)
        var password by mutableStateOf("Xq7#vRt9-Mzw2")
        var assessment: CoreResult<PasswordFeedback>? = null
        var frameTime = 0L
        fun frame() {
            runCurrent()
            Snapshot.sendApplyNotifications()
            runCurrent()
            frameTime += 16_000_000L
            clock.sendFrame(frameTime)
            runCurrent()
        }
        try {
            composition.setContent { assessment = rememberPasswordAssessment(core, password) }
            runCurrent()
            assertNull(assessment)
            requests.getValue(password).complete(CoreResult.Value(PasswordFeedback(PasswordStrength.Strong, emptyList())))
            frame()
            assertTrue(assessment.acceptsPassword())

            password = "Aa729583𝔸"
            frame()
            assertNull(assessment, "the old verdict survived a password edit")
            assertFalse(assessment.acceptsPassword())
            requests.getValue(password).complete(CoreResult.Value(PasswordFeedback(PasswordStrength.Weak, listOf(PasswordHint.OneKind))))
            frame()
            assertFalse(assessment.acceptsPassword(), "the form must use the core verdict for supplementary Unicode letters")
        } finally {
            composition.dispose()
            recomposer.close()
            job.cancelAndJoin()
        }
    }
}
