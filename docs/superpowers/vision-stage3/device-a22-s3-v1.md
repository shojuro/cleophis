# Vision probe score: PASS

| Gate | Value | Requirement |
|---|---|---|
| exact | 18/18 | >= 17: total - 1 (fewer than 20 items) |
| accepted sign, operator or symbol errors | 0 | <= 0 |
| refusals correct | 2 | >= 2 |
| p50 total_ms | 8791 | <= 10000 |
| p95 total_ms | 10604 | <= 12000 |
| VmHWM max (KiB) | 389292 | <= 1258291 |

## Items

| id | exact | CER | sign | operator | symbol | weighted | expected | got | tutor |
|---|---|---|---|---|---|---|---|---|---|
| q01 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | review | no |
| q02 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q03 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | review | no |
| q04 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | review | no |
| q05 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q06 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | review | no |
| q07 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q08 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q09 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q10 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | review | no |
| q11 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q12 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q13 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q14 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | review | no |
| q15 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q16 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q17 | yes | 0.000 | 0 | 0 | 0 | 0 | refuse | refuse | no |
| q18 | yes | 0.000 | 0 | 0 | 0 | 0 | refuse | refuse | no |

## Memory of the bench process

Snapshot: captured. The bench is a plain binary, so this is the number without a WebView.

```
# dumpsys meminfo 19297 (vision_bench, ~20s into the run)
Applications Memory Usage (in Kilobytes):
Uptime: 266630876 Realtime: 280818489
                   Pss  Private  Private     Swap      Rss     Heap     Heap     Heap
                 Total    Dirty    Clean    Dirty    Total     Size    Alloc     Free
                ------   ------   ------   ------   ------   ------   ------   ------
  Native Heap   206240   206240        0        0   206240        0        0        0
  Dalvik Heap        0        0        0        0        0        0        0        0
        Stack      112      112        0        0      112                           
    Other dev        0        0        0        0      368                           
     .so mmap     1318      144     1156        0     3068                           
   Other mmap   174771      180   174564        0   175468                           
      Unknown      848      848        0        0      848                           
        TOTAL   383289   207524   175720        0   386104        0        0        0
 
 App Summary
                       Pss(KB)                        Rss(KB)
                        ------                         ------
           Java Heap:        0                              0
         Native Heap:   206240                         206240
                Code:     1300                           3068
               Stack:      112                            112
            Graphics:        0                              0
       Private Other:   175592
              System:       45
             Unknown:                                  176684
 
           TOTAL PSS:   383289            TOTAL RSS:   386104      TOTAL SWAP (KB):        0
```

Max VmHWM across records: 389292 kB
