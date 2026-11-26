package io.github.stratayun.paranoic.platform

/**
 * Image formats the core decoder cannot read (some WebP variants, HEIC/AVIF under any
 * extension) are handed to the platform decoder when one is installed (Android).
 * Input: the file bytes; output: a PNG no larger than [side] pixels, or null.
 */
object PlatformImageFallback {
    @Volatile var decode: ((ByteArray, Int) -> ByteArray?)? = null
}
