---
title: HTTP media
description: SoftAP /v2 file fetch, media list, delete, and favorite/star.
---

Once the phone is on the camera [SoftAP](../wifi/), stills and clips are fetched over HTTP. Listing, delete, and star stay on [DUML](../commands/).

## File fetch

```text
GET http://192.168.2.1/v2?storage={0|1}&path=
```

| | |
| --- | --- |
| Thumbnails | `MISC/THM/…/.scr` (JPEG) |
| Internal handle bit `0x40000000` | storage 1 |
| Pocket 3 (single microSD) | always storage 0, even when the handle has that bit |

Do not put camera paths or captured filenames that include secrets into issues.

## List

| set/cmd | meaning | notes |
| --- | --- | --- |
| `0x00/0x26` | media list request | cursor `@10` u32-LE; ctr `@4`. Trigger `4a040e10`. Newest page needs no playback (list it even if `0x02/0x0c` ACKs E0); older pages do. |
| `0x00/0x27` | media list chunks | `[10B sub][chunk]`; subtype `01` is data. Concat in arrival order → CompositePack. |
| `0x02/0x0c` | enter/exit playback | `01 01 00 01` / `01 01 00 00`. Hold with `0x00/0x88` ~1 Hz. Do not poll `0x02/0x8E` while held. The reply means received, not entered: confirm on bit 30 of `0x02/0x80`. |
| `0x01/0x01` | Pocket 3 playback entry | Notify, no reply. When `0x02/0x0c` answers `E0`: `03 00000000 04000000 07 01` ~6 frames at ~20 Hz, then `00 00000000 04000000 04 01` at ~20 Hz until the playback bit sets (~350 ms). No exit; the body returns to capture on its own after the link drops. Learned from the public Osmosis notes (§13b). |

A store is not mounted the instant playback is confirmed: a list sent too early answers
`d8` and opens an empty transfer. Allow ~1.7 s after the bit sets, or re-ask a store that
came back empty.

## Delete and star

| set/cmd | meaning | notes |
| --- | --- | --- |
| `0x00/0x28` | delete media | `[count][handle:u32][counter:u32] 00 [count:u32] 01 01 00 00`. Do not re-send. |
| `0x02/0xBF` | favorite / star | `01 01 [handle][counter] 00 [on] 00 00 00`. Nano star byte `== 1` only. |
