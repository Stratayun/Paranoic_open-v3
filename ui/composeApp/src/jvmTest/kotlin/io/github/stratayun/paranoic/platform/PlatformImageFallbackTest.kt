package io.github.stratayun.paranoic.platform

import kotlin.test.*

class PlatformImageFallbackTest {
    @Test fun panoramasAreSampledByTheirLongestSide() {
        assertEquals(64, PlatformImageFallback.sampleSize(65_536, 1, 1024))
        assertEquals(64, PlatformImageFallback.sampleSize(1, 65_536, 1024))
        assertEquals(4, PlatformImageFallback.sampleSize(4097, 20, 2048))
    }

    @Test fun hostileHeaderDimensionsAndInvalidTargetsAreRejected() {
        listOf(0 to 10, -1 to 10, Int.MAX_VALUE to 1, 65_536 to 65_536).forEach { (w, h) ->
            assertFailsWith<IllegalArgumentException> { PlatformImageFallback.sampleSize(w, h, 1024) }
        }
        listOf(-1, 0, 4097, Int.MAX_VALUE).forEach { side ->
            assertFailsWith<IllegalArgumentException> { PlatformImageFallback.sampleSize(10, 10, side) }
        }
    }

    @Test fun sampledDimensionsFitThePixelBudgetWithoutUpscaling() {
        for ((w, h) in listOf(8192 to 8192, 65_536 to 1024, 17 to 19, 1 to 1)) {
            for (side in listOf(1, 31, 1024, 4096)) {
                val sample = PlatformImageFallback.sampleSize(w, h, side)
                val x = (w.toLong() + sample - 1) / sample
                val y = (h.toLong() + sample - 1) / sample
                assertTrue(x in 1..side.toLong() && y in 1..side.toLong())
                assertTrue(x * y * 4 <= PngBounds.MAX_PIXEL_BYTES)
            }
        }
        assertEquals(1, PlatformImageFallback.sampleSize(17, 19, 1024))
    }
}
