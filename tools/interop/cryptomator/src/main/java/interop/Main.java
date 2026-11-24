package interop;

import org.cryptomator.cryptofs.CryptoFileSystemProperties;
import org.cryptomator.cryptofs.CryptoFileSystemProvider;
import org.cryptomator.cryptolib.api.CryptorProvider;
import org.cryptomator.cryptolib.api.Masterkey;
import org.cryptomator.cryptolib.api.MasterkeyLoader;
import org.cryptomator.cryptolib.common.MasterkeyFileAccess;

import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.security.SecureRandom;
import java.util.Arrays;

/**
 * Cryptomator interoperability fixtures built with Cryptomator's own libraries
 * (cryptofs/cryptolib). create: new vaults with a known tree. verify: read the
 * tree that Paranoic wrote into the same vaults.
 */
public class Main {
    static final String PASSWORD = "interop-password";
    static final String LONG = "very long file name ".repeat(12).trim() + ".txt";
    static final String LONG_DIR = "long directory ".repeat(15).trim();

    static byte[] pattern(int length, int seed) {
        byte[] out = new byte[length];
        for (int i = 0; i < length; i++) out[i] = (byte) (i * 31 + seed);
        return out;
    }

    static java.nio.file.FileSystem open(Path vault) throws Exception {
        MasterkeyFileAccess access = new MasterkeyFileAccess(new byte[0], SecureRandom.getInstanceStrong());
        MasterkeyLoader loader = keyId -> access.load(vault.resolve("masterkey.cryptomator"), PASSWORD);
        return CryptoFileSystemProvider.newFileSystem(vault, CryptoFileSystemProperties.cryptoFileSystemProperties().withKeyLoader(loader).build());
    }

    static void create(Path vault, CryptorProvider.Scheme scheme) throws Exception {
        Files.createDirectories(vault);
        SecureRandom random = SecureRandom.getInstanceStrong();
        Masterkey key = Masterkey.generate(random);
        new MasterkeyFileAccess(new byte[0], random).persist(key, vault.resolve("masterkey.cryptomator"), PASSWORD);
        MasterkeyLoader loader = keyId -> key.copy();
        CryptoFileSystemProperties props = CryptoFileSystemProperties.cryptoFileSystemProperties()
            .withKeyLoader(loader).withCipherCombo(scheme).build();
        CryptoFileSystemProvider.initialize(vault, props, URI.create("masterkeyfile:masterkey.cryptomator"));
        try (java.nio.file.FileSystem fs = CryptoFileSystemProvider.newFileSystem(vault, props)) {
            Path root = fs.getPath("/");
            Files.write(root.resolve("empty.txt"), new byte[0]);
            Files.write(root.resolve("hello.txt"), "hello from cryptomator".getBytes(StandardCharsets.UTF_8));
            Files.write(root.resolve("multi-chunk.bin"), pattern(100_000, 5));
            Files.write(root.resolve(LONG), pattern(1000, 6));
            Path nested = Files.createDirectories(root.resolve("dir").resolve("nested"));
            Files.write(nested.resolve("deep.txt"), "deep".getBytes(StandardCharsets.UTF_8));
            Path longDir = Files.createDirectory(root.resolve(LONG_DIR));
            Files.write(longDir.resolve("inside.txt"), "inside long dir".getBytes(StandardCharsets.UTF_8));
            Files.createSymbolicLink(root.resolve("link"), fs.getPath("dir/nested/deep.txt"));
        }
        System.out.println("created " + vault + " " + scheme);
    }

    static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    static void verify(Path vault) throws Exception {
        try (java.nio.file.FileSystem fs = open(vault)) {
            Path root = fs.getPath("/");
            check(Arrays.equals(Files.readAllBytes(root.resolve("from-paranoic.bin")), pattern(70_000, 9)), "from-paranoic.bin content");
            check(Arrays.equals(Files.readAllBytes(root.resolve("dir").resolve("Paranoic folder").resolve(LONG + ".paranoic")), pattern(500, 10)), "long-named import");
            check(Files.exists(root.resolve("renamed-hello.txt")) && !Files.exists(root.resolve("hello.txt")), "rename");
            check(Arrays.equals(Files.readAllBytes(root.resolve("renamed-hello.txt")), "hello from cryptomator".getBytes(StandardCharsets.UTF_8)), "renamed content");
            check(!Files.exists(root.resolve("multi-chunk.bin")), "deleted file");
            check(!Files.exists(root.resolve(LONG_DIR)), "deleted long-named directory");
            check(Files.isDirectory(root.resolve("dir").resolve("nested")), "untouched directory");
        }
        System.out.println("verified " + vault);
    }

    public static void main(String[] args) throws Exception {
        Path base = Paths.get(args[1]);
        switch (args[0]) {
            case "create" -> {
                create(base.resolve("v8-gcm"), CryptorProvider.Scheme.SIV_GCM);
                create(base.resolve("v8-ctrmac"), CryptorProvider.Scheme.SIV_CTRMAC);
            }
            case "verify" -> { verify(base.resolve("v8-gcm")); verify(base.resolve("v8-ctrmac")); }
            default -> throw new IllegalArgumentException(args[0]);
        }
    }
}
