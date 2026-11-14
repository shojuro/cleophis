# Vision probe score: PASS

| Gate | Value | Requirement |
|---|---|---|
| exact | 22/22 | >= 20: total - 2 (20 or more items) |
| accepted sign, operator or symbol errors | 0 | <= 0 |
| refusals correct | 4 | >= 4 (every refuse item) |
| p50 total_ms | 8733 | <= 10000 |
| p95 total_ms | 9870 | <= 12000 |
| VmHWM max (KiB) | 389188 | <= 1258291 |

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
| q19 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q20 | yes | 0.000 | 0 | 0 | 0 | 0 | accept | accept | no |
| q21 | yes | 0.000 | 0 | 0 | 0 | 0 | refuse | refuse | no |
| q22 | yes | 0.000 | 0 | 0 | 0 | 0 | refuse | refuse | no |

## Memory of the bench process

Snapshot: captured. The bench is a plain binary, so this is the number without a WebView.

```
# dumpsys meminfo 5020 (vision_bench, ~20s into the run)
Applications Memory Usage (in Kilobytes):
Uptime: 273322485 Realtime: 287510098
                   Pss  Private  Private     Swap      Rss     Heap     Heap     Heap
                 Total    Dirty    Clean    Dirty    Total     Size    Alloc     Free
                ------   ------   ------   ------   ------   ------   ------   ------
  Native Heap   206224   206224        0        0   206224        0        0        0
  Dalvik Heap        0        0        0        0        0        0        0        0
        Stack      108      108        0        0      108                           
    Other dev        0        0        0        0      388                           
     .so mmap     1269      144     1112        0     2916                           
   Other mmap   174773      180   174572        0   175464                           
      Unknown      848      848        0        0      848                           
        TOTAL   383222   207504   175684        0   385948        0        0        0
 
 App Summary
                       Pss(KB)                        Rss(KB)
                        ------                         ------
           Java Heap:        0                              0
         Native Heap:   206224                         206224
                Code:     1256                           2916
               Stack:      108                            108
            Graphics:        0                              0
       Private Other:   175600
              System:       34
             Unknown:                                  176700
 
           TOTAL PSS:   383222            TOTAL RSS:   385948      TOTAL SWAP (KB):        0
```

Max VmHWM across records: 389188 kB
