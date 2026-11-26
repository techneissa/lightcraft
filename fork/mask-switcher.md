# Switch masks without scrolling

List: The Masking panel's mask list stays at the top with a thumbnail per mask, and `,` / `.` step between masks. Branch `feat/401-mask-switcher`.

Fork issue #401. The title and the mask list (`panels/mask_list.rs`) are drawn above the right panel's scroll area, the rest of the Masking panel (`panels/masking.rs`) scrolls beneath. One row per mask: a coverage thumbnail (`panels/mask_thumbs.rs`: the pipeline's mask planes at 64 px, made on a worker when a mask's shape or the crop changes) and the whole name, wrapped. The list shows about 5 rows, then scrolls on its own. The picked row has a bar at its left edge and a heavier name.

Picking another mask (a row, a key, a pin, `mask.select`) scrolls the panel to that mask's Add · Subtract · Intersect control, parts and sliders. Adding, duplicating or deleting a mask leaves the scroll alone.

`mask.next` and `mask.previous` (engine commands) wrap around the list's ends; their keys `.` and `,` work in the Masking tool only. The list is a list box to screen readers: with a row focused, Up and Down move to the neighbouring mask and the focus goes along.
