# Emoji dataset

`emoji.json` is a curated, checked-in subset of Unicode emoji data with
CLDR-style annotations. It is embedded with `include_str!`; runtime lookup never
performs a network request.

## Metadata

| Field | Value |
|---|---|
| Dataset version | `emoji-1.0.0` |
| Unicode release | `16.0` |
| License | Unicode License v3 (see `LICENSE`) |
| License URL | <https://www.unicode.org/license.txt> |
| Entries | 139 curated emoji and symbol entries |
| Checksum | FNV-1a 64, recorded in the `checksum` field |

Source: a curated subset of Unicode 16.0 emoji data and CLDR 46 annotations.
Character names and CLDR-style keywords are the primary source data. Skin-tone
and text-presentation variants are explicit entries so the host UI can offer
distinguishable accessible names.

## Checksum rule

The checksum is FNV-1a 64 (offset basis `0xcbf29ce484222325`, prime
`0x100000001b3`) over the UTF-8 bytes of the canonical compact JSON document
with the `checksum` field set to the empty string. Field order is:

```text
version, unicode_version, source, license, license_url, checksum, entries
```

Each entry is:

```text
id, name, keywords, group, base, variants
```

Each variant is:

```text
name, grapheme
```

Loading recomputes this value with `serde_json` and fails with
`DatasetError::ChecksumMismatch` when the stored checksum, the recomputed
checksum and `PINNED_CHECKSUM` disagree. In particular:

- `load_from_json` always requires `PINNED_CHECKSUM`. Tampered data with a
  recomputed self-consistent checksum is rejected.
- `load_from_json_with_checksum(json, expected)` is the fixture/regeneration
  path that pins an explicit non-production checksum.
- `compute_checksum_for_json` recomputes the canonical value without loading.

Caller-supplied input is bounded by `MAX_DATASET_BYTES` (1 MiB) and
`MAX_DATASET_ENTRIES` (10,000). FNV-1a 64 is an integrity pin, not a
cryptographic signature; it is not intended to resist a deliberate collision
attack.

## Update process

1. Edit the curated entries, keeping every `base` and variant `grapheme` at
   exactly one extended grapheme cluster.
2. Recompute the checksum with the rule above and write it into `checksum`.
3. Update `PINNED_CHECKSUM` in `crates/emoji/src/dataset.rs` and the metadata
   table above.
4. Run `cargo test -p ansa-emoji --offline`. The grapheme corpus test rejects
   any base or variant that segmented into more than one cluster.

No currency, date or time-zone data is present. The dataset contains no
network URLs used at query time.
