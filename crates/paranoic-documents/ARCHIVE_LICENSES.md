# Archive dependencies

7z: sevenz-rust2 0.7.0, Apache-2.0; upstream
https://github.com/hasenbanck/sevenz-rust2/tree/v0.7.0 . This version declares
Rust 1.74.0, compatible with this workspace's Rust 1.87 baseline. Preserve its
Apache license and transitive notices in distributed packages.

RAR: not supported (removed 2026-11-12); no UnRAR code is linked or distributed.

Office viewers (DOCX/XLSX/ODS): quick-xml 0.39.4, MIT; own parsers, no
calamine (its Range allocation follows sheet dimensions, unsuitable for
hostile input). ZIP container read through the existing zip 2.4.2 (MIT).

Media (2.7): image 0.25.9 (MIT/Apache-2.0) with png, jpeg (zune-jpeg,
MIT/Apache-2.0/Zlib), gif, webp (image-webp), bmp only; no native codecs.
