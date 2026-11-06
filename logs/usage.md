# Token usage per session

Summed from the exported transcripts by `scripts/token-usage.sh`. Each API response counted once.
"All tokens" = input + cache write + cache read + output.

| Transcript | Input | Cache write | Cache read | Output | Responses | All tokens | Models (responses) |
|---|---|---|---|---|---|---|---|
| architect-005f9e94 | 140 | 259,348 | 7,390,482 | 67,220 | 70 | 7,717,190 | claude-opus-5-5 (70) |
| executor-ec739230 (incl. subagents) | 3,600 | 3,818,540 | 233,453,371 | 1,253,603 | 1,778 | 238,529,114 | claude-opus-5-5 (1778) |

### executor-ec739230 subagents

| Transcript | Input | Cache write | Cache read | Output | Responses | All tokens | Models (responses) |
|---|---|---|---|---|---|---|---|
| executor-ec739230-subagents/agent-a0a0b39470910a9fb.jsonl | 268 | 173,324 | 13,822,862 | 84,494 | 132 | 14,080,948 | claude-opus-5-5 (132) |
| executor-ec739230-subagents/agent-a138e079d213c3075.jsonl | 70 | 95,123 | 2,205,342 | 37,547 | 34 | 2,338,082 | claude-opus-5-5 (34) |
| executor-ec739230-subagents/agent-a20d0764ff98570f8.jsonl | 164 | 100,723 | 5,725,824 | 45,433 | 81 | 5,872,144 | claude-opus-5-5 (81) |
| executor-ec739230-subagents/agent-a2782b722cdc1f6f2.jsonl | 14 | 22,508 | 103,603 | 3,441 | 7 | 129,566 | claude-opus-5-5 (7) |
| executor-ec739230-subagents/agent-a279b498b8a6fb0ad.jsonl | 106 | 85,544 | 2,946,615 | 31,396 | 53 | 3,063,661 | claude-opus-5-5 (53) |
| executor-ec739230-subagents/agent-a3676904e27f8b777.jsonl | 16 | 21,504 | 105,890 | 3,403 | 8 | 130,813 | claude-opus-5-5 (8) |
| executor-ec739230-subagents/agent-a3f4bb9ea8790482b.jsonl | 14 | 36,447 | 158,350 | 4,956 | 7 | 199,767 | claude-opus-5-5 (7) |
| executor-ec739230-subagents/agent-a43403a18caac5e09.jsonl | 114 | 80,320 | 2,676,266 | 43,784 | 56 | 2,800,484 | claude-opus-5-5 (56) |
| executor-ec739230-subagents/agent-a4ec101f887cf580c.jsonl | 64 | 52,575 | 1,250,858 | 19,706 | 32 | 1,323,203 | claude-opus-5-5 (32) |
| executor-ec739230-subagents/agent-a554fed613d42ef0a.jsonl | 22 | 48,842 | 366,519 | 7,397 | 11 | 422,780 | claude-opus-5-5 (11) |
| executor-ec739230-subagents/agent-a5bd8848570c3a983.jsonl | 30 | 44,390 | 498,898 | 9,240 | 15 | 552,558 | claude-opus-5-5 (15) |
| executor-ec739230-subagents/agent-a74f182e8b7063d5e.jsonl | 126 | 241,113 | 3,728,315 | 29,527 | 60 | 3,999,081 | claude-opus-5-5 (60) |
| executor-ec739230-subagents/agent-a77938bdf0ab1e4e2.jsonl | 42 | 41,146 | 677,446 | 11,910 | 21 | 730,544 | claude-opus-5-5 (21) |
| executor-ec739230-subagents/agent-a874da11c32c210d1.jsonl | 334 | 224,678 | 20,630,511 | 99,823 | 165 | 20,955,346 | claude-opus-5-5 (165) |
| executor-ec739230-subagents/agent-a8bb2f059d3e1bdc5.jsonl | 20 | 23,710 | 189,050 | 4,245 | 10 | 217,025 | claude-opus-5-5 (10) |
| executor-ec739230-subagents/agent-a90e81cb218647ab5.jsonl | 42 | 114,901 | 787,243 | 15,860 | 20 | 918,046 | claude-opus-5-5 (20) |
| executor-ec739230-subagents/agent-a9a49638850b24e41.jsonl | 22 | 48,565 | 339,511 | 10,147 | 11 | 398,245 | claude-opus-5-5 (11) |
| executor-ec739230-subagents/agent-aa2a37abbaaee51a6.jsonl | 22 | 28,812 | 218,478 | 6,327 | 11 | 253,639 | claude-opus-5-5 (11) |
| executor-ec739230-subagents/agent-aa85443cf651925b5.jsonl | 20 | 42,230 | 294,069 | 11,290 | 10 | 347,609 | claude-opus-5-5 (10) |
| executor-ec739230-subagents/agent-aa8ea7f2dce4476c9.jsonl | 20 | 46,726 | 313,933 | 9,099 | 10 | 369,778 | claude-opus-5-5 (10) |
| executor-ec739230-subagents/agent-ab059ccf04b4b29d3.jsonl | 26 | 63,112 | 559,618 | 8,118 | 13 | 630,874 | claude-opus-5-5 (13) |
| executor-ec739230-subagents/agent-ab46a0b4f861798d7.jsonl | 28 | 51,491 | 455,319 | 12,123 | 14 | 518,961 | claude-opus-5-5 (14) |
| executor-ec739230-subagents/agent-ab5228b1551910e25.jsonl | 188 | 159,313 | 8,924,146 | 71,725 | 93 | 9,155,372 | claude-opus-5-5 (93) |
| executor-ec739230-subagents/agent-aba441ec95ceb0df2.jsonl | 30 | 31,401 | 362,723 | 7,328 | 15 | 401,482 | claude-opus-5-5 (15) |
| executor-ec739230-subagents/agent-abe680645dcdbc3e9.jsonl | 28 | 45,190 | 435,326 | 9,958 | 14 | 490,502 | claude-opus-5-5 (14) |
| executor-ec739230-subagents/agent-ac23e34c0c2f986ed.jsonl | 82 | 99,318 | 2,676,528 | 59,373 | 41 | 2,835,301 | claude-opus-5-5 (41) |
| executor-ec739230-subagents/agent-ac36e1d7cfd83e9e3.jsonl | 156 | 114,645 | 5,585,085 | 41,132 | 78 | 5,741,018 | claude-opus-5-5 (78) |
| executor-ec739230-subagents/agent-ac4010c5885eed2e6.jsonl | 48 | 89,120 | 1,356,925 | 38,927 | 23 | 1,485,020 | claude-opus-5-5 (23) |
| executor-ec739230-subagents/agent-ac6a9df8a7f0a8803.jsonl | 24 | 45,156 | 394,637 | 9,450 | 12 | 449,267 | claude-opus-5-5 (12) |
| executor-ec739230-subagents/agent-ac6d00efa469edd0f.jsonl | 26 | 28,839 | 291,764 | 8,479 | 13 | 329,108 | claude-opus-5-5 (13) |
| executor-ec739230-subagents/agent-ac7f7360ebb2a342e.jsonl | 22 | 36,981 | 282,893 | 7,272 | 11 | 327,168 | claude-opus-5-5 (11) |
| executor-ec739230-subagents/agent-acda757498da8e62f.jsonl | 64 | 127,742 | 1,487,738 | 23,854 | 31 | 1,639,398 | claude-opus-5-5 (31) |
| executor-ec739230-subagents/agent-ad001d6aa1f486534.jsonl | 10 | 15,100 | 58,640 | 2,967 | 5 | 76,717 | claude-opus-5-5 (5) |
| executor-ec739230-subagents/agent-ad1b92e5697b683c7.jsonl | 26 | 34,196 | 324,975 | 5,912 | 13 | 365,109 | claude-opus-5-5 (13) |
| executor-ec739230-subagents/agent-ad24e8a2be3196f5d.jsonl | 118 | 146,837 | 2,919,188 | 26,678 | 53 | 3,092,821 | claude-opus-5-5 (53) |
| executor-ec739230-subagents/agent-ad35ab241d11cd954.jsonl | 48 | 66,270 | 1,201,452 | 27,438 | 24 | 1,295,208 | claude-opus-5-5 (24) |
| executor-ec739230-subagents/agent-ad48c768da4b9ac2b.jsonl | 120 | 103,014 | 3,540,074 | 50,979 | 58 | 3,694,187 | claude-opus-5-5 (58) |
| executor-ec739230-subagents/agent-ad561cb9c08ed324d.jsonl | 20 | 46,478 | 291,166 | 8,648 | 10 | 346,312 | claude-opus-5-5 (10) |
| executor-ec739230-subagents/agent-ad8a35b7e5ccc2480.jsonl | 24 | 38,754 | 330,376 | 8,168 | 12 | 377,322 | claude-opus-5-5 (12) |
| executor-ec739230-subagents/agent-adf85458b11336286.jsonl | 10 | 18,438 | 60,735 | 2,155 | 5 | 81,338 | claude-opus-5-5 (5) |
| executor-ec739230-subagents/agent-ae1578eaf5ba219f5.jsonl | 32 | 31,477 | 414,973 | 8,795 | 16 | 455,277 | claude-opus-5-5 (16) |
| executor-ec739230-subagents/agent-ae1bf62eb14278f2e.jsonl | 28 | 53,676 | 582,251 | 10,225 | 14 | 646,180 | claude-opus-5-5 (14) |
| executor-ec739230-subagents/agent-ae4071a9857319cb4.jsonl | 172 | 192,893 | 10,458,957 | 68,767 | 86 | 10,720,789 | claude-opus-5-5 (86) |
| executor-ec739230-subagents/agent-ae505d00ce0732095.jsonl | 10 | 16,753 | 47,573 | 3,015 | 5 | 67,351 | claude-opus-5-5 (5) |

