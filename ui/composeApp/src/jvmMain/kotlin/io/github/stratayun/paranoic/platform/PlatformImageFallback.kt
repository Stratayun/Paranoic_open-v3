package io.github.stratayun.paranoic.platform

/**
 * Image formats the core decoder cannot read (some WebP variants, HEIC/AVIF under any
 * extension) are handed to the platform decoder when one is installed (Android).
 * Input: the file bytes; output: a PNG no larger than [side] pixels, or null.
 */
object PlatformImageFallback {
    @Volatile var decode: ((ByteArray, Int) -> ByteArray?)? = null

    internal const val MAX_INPUT_BYTES = 32 * 1024 * 1024
    private const val MAX_SOURCE_DIMENSION = 65_536
    private const val MAX_SOURCE_PIXELS = 64L * 1024 * 1024

    /** Check hostile header dimensions before allocating pixels, including panoramas. */
    internal fun sampleSize(width: Int, height: Int, side: Int): Int {
        require(side in 1..PngBounds.MAX_DIMENSION) { "Image target size limit" }
        require(width in 1..MAX_SOURCE_DIMENSION && height in 1..MAX_SOURCE_DIMENSION &&
            width.toLong() * height <= MAX_SOURCE_PIXELS) { "Image source size limit" }
        var sample = 1
        // Round up: a decoder may round sampled dimensions up, not down.
        while ((width.toLong() + sample - 1) / sample > side ||
            (height.toLong() + sample - 1) / sample > side) sample *= 2
        return sample
    }
}
