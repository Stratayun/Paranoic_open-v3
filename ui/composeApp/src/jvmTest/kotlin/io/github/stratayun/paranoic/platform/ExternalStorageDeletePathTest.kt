package io.github.stratayun.paranoic.platform

import kotlin.test.*

class ExternalStorageDeletePathTest {
    @Test fun acceptsOnlyDescendantsOnTheSameVolume() {
        val path = ExternalStorageDeletePath.target("primary:Documents/Сейф", "primary:Documents/Сейф/файл.txt")
        assertEquals("primary", path.volume)
        assertEquals(listOf("Documents", "Сейф", "файл.txt"), path.parts)
        assertEquals("abcd-1234", ExternalStorageDeletePath.target("ABCD-1234:", "abcd-1234:folder/file").volume)
    }
    @Test fun refusesRootSiblingPrefixesAndForeignVolumes() {
        for ((root, target) in listOf("primary:" to "primary:", "primary:a" to "primary:a",
            "primary:a" to "primary:ab/file", "primary:a" to "primary:file",
            "primary:a" to "other:a/file", "primary:A" to "primary:a/file")) {
            assertFailsWith<IllegalArgumentException> { ExternalStorageDeletePath.target(root, target) }
        }
    }
    @Test fun refusesTraversalAndNoncanonicalComponents() {
        for (path in listOf("a/../file", "a/./file", "/file", "a//file", "a/", "a\\file", "a\u0000file", "a:b")) {
            assertFailsWith<IllegalArgumentException> { ExternalStorageDeletePath.target("primary:", "primary:$path") }
        }
    }
    @Test fun refusesMalformedAndOversizedIds() {
        for (id in listOf("primary", ":file", "../:file", "primary:" + "x".repeat(256),
            "primary:" + List(257) { "x" }.joinToString("/"), "primary:" + "x".repeat(8192))) {
            assertFailsWith<IllegalArgumentException> { ExternalStorageDeletePath.parse(id) }
        }
    }
}
