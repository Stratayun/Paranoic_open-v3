package io.github.stratayun.paranoic.platform

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import java.io.ByteArrayOutputStream

/** Bounded fallback for formats unavailable in the Rust renderer. Called under PngDecodeBudget. */
internal object AndroidImageFallback {
    fun decode(bytes: ByteArray, side: Int): ByteArray? {
        require(bytes.size in 1..PlatformImageFallback.MAX_INPUT_BYTES) { "Image byte limit" }
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        val sample = PlatformImageFallback.sampleSize(bounds.outWidth, bounds.outHeight, side)
        val full = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply {
            inSampleSize = sample
            inScaled = false
            inPreferredConfig = Bitmap.Config.ARGB_8888
        }) ?: return null
        return try {
            require(full.width in 1..side && full.height in 1..side &&
                full.allocationByteCount.toLong() <= PngBounds.MAX_PIXEL_BYTES) { "Image allocation limit" }
            WipingOutput().use { out ->
                check(full.compress(Bitmap.CompressFormat.PNG, 100, out)) { "PNG encoding failed" }
                val png = out.toByteArray()
                try { PngBounds.read(png); png }
                catch (error: Exception) { png.fill(0); throw error }
            }
        } finally {
            full.recycle()
        }
    }

    private class WipingOutput : ByteArrayOutputStream() {
        override fun close() { buf.fill(0); reset(); super.close() }
    }
}
