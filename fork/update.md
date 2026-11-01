# Update the app

1. Click **Update LightCraft** in Applications. (Same thing in Terminal:
   `~/scripts/craft/update-lightcraft.sh`; `update-craft-apps.sh` updates every ArtCraft app.)
2. It downloads the newest app GitHub built ([github-builds.md](github-builds.md); seconds), when that build has everything the Mac's `custom`
   has. Official changes made since that build come with the next one (daily at 08:17, and after every push).
3. Otherwise, or with `update-lightcraft.sh --latest`, it gets the official update, merges it into `custom` and
   backs `custom` up to this fork. Then it installs the app: a download of seconds when GitHub has already built
   that version, else a build on the Mac (1 to 10 minutes).
4. If it stops with "the update touches the same code as your edits", nothing was changed. Ask an agent to
   "update LightCraft" (it follows the `craftupdate` skill).

The script is `update-craft.sh` in the same folder, shared by every ArtCraft app; it keeps your edits in any
app whose repo has a `custom` branch. The official release download (`update-lightcraft-release.sh`) installs
the official app without these features.
