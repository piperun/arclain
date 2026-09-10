# Owned archive fixtures

All payloads are harmless test data and are never executed. Game/game.exe and
Game/data/data.bin each contain 8,192 deterministic random bytes generated with
`random.Random(7123)` in that order. 7-Zip 26.03 produced ZIP and 7z fixtures.
`solid.7z` uses LZMA2 solid mode; its uncompressed header dictionary property
was raised to 0x20 (256 MiB), with header CRCs recomputed, because the encoder
otherwise reduces dictionaries for small input. BCJ2 uses three LZMA2 streams.

The 92-byte Copy-solid interleaving fixture contains a=A, empty (zero bytes),
b=B. Unicode, duplicate, unsafe and invalid-UTF16 variants mutate its names
and recompute 7z header CRCs. Unsupported-method changes the Copy coder ID.
CRC variants deliberately damage checked bytes. The encrypted fixture uses
the public test password `fixture-password` and keeps its names visible.

RAR4/RAR5 fixtures are minimal stored headers and the 22-byte payload
`benign archive fixture`. Header CRCs and file CRCs are constructed directly.
Refusal variants mutate original names/host fields (recomputing header CRCs)
or payload bytes. No commercial archive or user game content is included.

WinRAR 7.13 produced the `rar5-winrar*` fixtures from the same 22-byte
payload: the default shape (an extra area on every header, a locator
record in the main header, a file time record on the entry), a Unicode
name, `-p` file encryption, `-hp` header encryption, `-am` archive
metadata, and `-s -rr -qo+` (solid, with a recovery record and quick-open
records as service headers). The RAR4 variants are hand-built, since WinRAR
7 no longer writes RAR4: an extended time record, Unicode names (one using
the two-byte code alone, one using the run, corrected-run, high-byte and
plain codes), a password with a salt, encrypted headers, and comment and
recovery sub-blocks around the entry. Their end headers carry the
skippable mark WinRAR sets, and the sub-blocks fixture's carries the
archive data CRC as well. `delta.7z` holds `sound.bin`, 4,096 bytes where
byte i is i*7 mod 256, which 7-Zip 26.03 compressed with the Delta:4
filter before LZMA2. Encrypted fixtures use the public test password
`fixture-password`.

| Fixture | SHA-256 |
| --- | --- |
| bad-data-crc-rar4.rar | 51f8b8b7418e3b4bd963f6e78bc261accb82ab21148ffb2bf058cc877ac56391 |
| bad-data-crc-rar5.rar | dd3a8c8ed51a2c7ffed2ef308610a14029c937cfe36dde680fe97e0d2fe64410 |
| bad-data-crc.7z | 17a47e0ef98c4f668ef7456e91b95f044e47dc6b5b3fe734d5e206764226f871 |
| bad-header-crc.7z | ef4ae3edf80517c62a77456709493e0df3b15bcef707473acf668c5559f45867 |
| bcj2.7z | 3113eb3ba935fbe736a4aac331b5ff7e6e38dd0aaa48572be7f36fbcad3afd9c |
| delta.7z | f5c0b0ecc3dad8c6a1ff0e9325b025e09dc1ab22e3b0c215f05f151643f41001 |
| duplicate-name.7z | 3a20cb73ce650df28da4a2d5b4e11e4a9f4b949a51afa02801322b595f4b92f4 |
| encrypted.7z | ffc6c80df6fecda0c9a214e7b5efc880626e3f2897fc985256bcc0f8e51af14b |
| game-rar4.rar | 0feec6e6f8ebf444227389f597cb675a0a981ef9645b13d712672fb51f7ffe7d |
| game-rar5.rar | 470376d8677de5f056581e8d3f92e7a8da5711dfea937a666ac553ecc5f720a4 |
| game.zip | 84fe66d9a10ae08a196090873e8e131c2b1c618f19d98354f2a971217143185a |
| host255.rar | 521d444442f8acf58735666cb7ea8fb992ae50c09a0be738feac12d7c9638ff3 |
| host6.rar | dd73d23ceed2b00f0fadfa42ee088a30b50d7ffe36821f134f491fd118d49bca |
| interleaved.7z | f04fd429aa18723ebb302a5e2fedf76ec46f6c98db69fc350dd022b67bdcd620 |
| invalid-utf16.7z | 455f38fed720d22f057cabcc9f0d1480f404696d04793f18ea1be97db211f757 |
| rar4-encrypted-headers.rar | f77a9072ee79ac5a0a9b17d909d077ae8553b13ebe3c04312ba89d72dc24b494 |
| rar4-exttime.rar | 6de77e8aa70985438c43992f6454af84e58e8aa104d6383dbdc16c4eeb63ae9c |
| rar4-password.rar | 0ca5f1612ba347084b0dc8e66e711bdcf7f5f2975f151501974d26c1f9caa884 |
| rar4-subblocks.rar | f38219e2ce0415376b0c1626666b66a92b191f27a7018d9dca00c88b95534ca7 |
| rar4-unicode-runs.rar | 7f288e4ae595927e13f46bd8bb831b130da01cfa4b7e7f72f34f21bd123cb0e9 |
| rar4-unicode.rar | f04b30548bfbf6ffacd893a85d321c8c9840b5d6f9b370569fb14e8a402f4d24 |
| rar4-unproven-encoding.rar | b51964ef6f80b46ec6c24268bb8e4304c5a401dbaa7654f941706ea2cf839218 |
| rar5-backslash.rar | 263d83f2cd4935aa108460ba0e0dffabf760fb48976163a11568a4e749a95e6f |
| rar5-colon.rar | 7676dbd648a458368c174de42db22df2ff1799cc7c2203ba6033bc12f3d9d6d9 |
| rar5-invalid-utf8.rar | a7dc2ab16960b9e4795a00df1c43c5e9234e71a28810f187fb35ee81142eae09 |
| rar5-nul.rar | f30e31c1aacd35b015f4b3d5f868d86c24be2c6caf1075d11a59588c24ece348 |
| rar5-overlong.rar | c3daab294a83b589751f39db356716c1192cceeec95bd9055dac35dc4786447c |
| rar5-traversal.rar | 2bec06c72ad927ca489eba146e0cc43c7b1fe5f9d409caba0d30f5017a18841e |
| rar5-winrar-encrypted-headers.rar | ca93bc4461d405c732cccc04193de12aed61a9b89663233c4e4581bac0bee76b |
| rar5-winrar-encrypted.rar | ffcf68c22f95007be83527b5d842c79b3739be83faa811fbef6520995ebeafa8 |
| rar5-winrar-metadata.rar | c0d9178eddf531356ad1e1bbb548953b8333453f93d09a651fb5cdf0cfb2428f |
| rar5-winrar-solid-rr.rar | 6fe2978a8b5b20db70eee877440a0c71877d99cebc8d2a807d7f1f0ff46c6748 |
| rar5-winrar-unicode.rar | 4e3bae4fce1c2a9fbda293d9d32994b239b714dd6049db92aaea77873030d6ad |
| rar5-winrar.rar | bd340a8d0d48610d0a15ab25fda7e8d2c5e29a8d274331cf0d160f7e0c4422c3 |
| solid.7z | cd9afbc380b44fb845745ced3f4a56839a4f76194c027356f2798f1b3f7985cc |
| unicode.7z | f3d89e5c5da70ae9f2d3540a65c831e323fe5d4b981600e41e0acc925d7ccb6b |
| unsafe-name.7z | 27efa71f6212f3f54bc5d7e0a661178b7e9653afa524de334f85bf86bd27e132 |
| unsupported-method.7z | bcdd0467082328139c517fbec0c61db45639f1775c839b91052c88c814f33d5b |
