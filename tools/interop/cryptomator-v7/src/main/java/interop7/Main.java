package interop7;

import org.cryptomator.cryptofs.CryptoFileSystemProperties;
import org.cryptomator.cryptofs.CryptoFileSystemProvider;

import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.Arrays;

/** Vault format 7 fixtures via cryptofs 1.9.x (the last line writing format 7). */
public class Main {
    static final String PASSWORD = "interop-password";
    static final String LONG = "very long file name ".repeat(12).trim() + ".txt";

    static byte[] pattern(int length, int seed) {
        byte[] out = new byte[length];
        for (int i = 0; i < length; i++) out[i] = (byte) (i * 31 + seed);
        return out;
    }

    static FileSystem open(Path vault) throws Exception {
        return CryptoFileSystemProvider.newFileSystem(vault, CryptoFileSystemProperties.cryptoFileSystemProperties().withPassphrase(PASSWORD).build());
    }

    public static void main(String[] args) throws Exception {
        Path vault = Paths.get(args[1]).resolve("v7");
        if (args[0].equals("create")) {
            Files.createDirectories(vault);
            CryptoFileSystemProvider.initialize(vault, "masterkey.cryptomator", PASSWORD);
            try (FileSystem fs = open(vault)) {
                Path root = fs.getPath("/");
                Files.write(root.resolve("hello.txt"), "hello from cryptomator 7".getBytes(StandardCharsets.UTF_8));
                Files.write(root.resolve("multi-chunk.bin"), pattern(100_000, 5));
                Files.write(root.resolve(LONG), pattern(1000, 6));
                Files.write(Files.createDirectories(root.resolve("dir")).resolve("deep.txt"), "deep".getBytes(StandardCharsets.UTF_8));
            }
            System.out.println("created " + vault);
        } else {
            try (FileSystem fs = open(vault)) {
                Path root = fs.getPath("/");
                if (!Arrays.equals(Files.readAllBytes(root.resolve("dir").resolve("from-paranoic.bin")), pattern(40_000, 9))) throw new AssertionError("v7 import");
                if (!Files.exists(root.resolve("renamed.txt")) || Files.exists(root.resolve("hello.txt"))) throw new AssertionError("v7 rename");
            }
            System.out.println("verified " + vault);
        }
    }
}
