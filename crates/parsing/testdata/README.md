# Test photos

No photo is committed. The pictures in the pull request come from these CC0 photos on Wikimedia Commons, downloaded
into the gitignored `corpus/parsing/` (licence checked with the Commons API, `LicenseShortName` = `CC0`, 2026-11-08):

| File | What it covers | Author | Source |
| --- | --- | --- | --- |
| `ChidoMpembaYoungAfricanWomanInPolitics.jpg` | single portrait, dark skin, locs, earrings | Chidompemba | <https://commons.wikimedia.org/wiki/File:ChidoMpembaYoungAfricanWomanInPolitics.jpg> |
| `Woman_in_Eyeglasses_3.jpg` | glasses, light skin | PxHere | <https://commons.wikimedia.org/wiki/File:Woman_in_Eyeglasses_(3).jpg> |
| `Woman_in_Eyeglasses_8.jpg` | half body, clothes | PxHere | <https://commons.wikimedia.org/wiki/File:Woman_in_Eyeglasses_(8).jpg> |
| `Treaty_of_Rome_anniversary_group_photograph_2017-03-25_01.jpg` | a group of 29 with small faces | Piotr Tracz, Kancelaria Premiera | <https://commons.wikimedia.org/wiki/File:Treaty_of_Rome_anniversary_group_photograph_2017-03-25_01.jpg> |
| `Lake_Mountain_Landscape.jpg` | no people | Bonnie Moreland | <https://commons.wikimedia.org/wiki/File:Lake_Mountain_Landscape.jpg> |

```bash
mkdir -p corpus/parsing && cd corpus/parsing
for f in ChidoMpembaYoungAfricanWomanInPolitics.jpg "Woman_in_Eyeglasses_(3).jpg" "Woman_in_Eyeglasses_(8).jpg" \
         Treaty_of_Rome_anniversary_group_photograph_2017-03-25_01.jpg Lake_Mountain_Landscape.jpg; do
  curl -sL -A "LightCraft-test-fetch/1.0" -o "${f//[()]/}" "https://commons.wikimedia.org/wiki/Special:FilePath/$f"
done
```

The unit tests need none of them: they use synthetic pictures.
