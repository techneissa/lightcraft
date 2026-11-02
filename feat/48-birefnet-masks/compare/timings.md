# Subject masks: SAM 3, BiRefNet, SAM 3 + BiRefNet Edges (issue #48)

M4 Pro, Metal, Mac under load (load average 40-80 from other work). Seconds are one `mask.add subject` each, models already loaded. SAM 3's analysis of the photo is cached for the combined run, so its seconds leave it out (cold, it is SAM 3 + BiRefNet). BiRefNet-matting: 5.7 s, run on three photos only.

| Photo | SAM 3 | BiRefNet | SAM 3 + BiRefNet Edges |
|---|---|---|---|
| feathers-1 | 8.0 s | 5.9 s | 7.1 s |
| feathers-2 | 8.0 s | 5.9 s | 6.9 s |
| fur-pet-2 | 8.1 s | 5.9 s | 6.9 s |
| fur-pet-3 | 7.9 s | 5.8 s | 6.9 s |
| hair-sky-2 | 7.5 s | 5.7 s | 6.4 s |
| hair-sky-3 | 7.6 s | 6.0 s | 6.6 s |
| lace-foliage-1 | found no subject | 5.7 s | 9.9 s |
| lace-foliage-4 | found no subject | 5.7 s | 8.7 s |
| lace-foliage-5 | found no subject | 5.7 s | 8.7 s |
| product-2 | found no subject | 5.8 s | 8.6 s |
| product-3 | found no subject | 5.8 s | 8.7 s |
| street-person-london | 7.4 s | 5.8 s | 6.5 s |
| street-person-umbrella | 7.5 s | 5.7 s | 6.4 s |
