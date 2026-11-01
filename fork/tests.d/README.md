# Tests of newer features

One file per feature, `<slug>.txt`, one command per line (blank lines and `#` comments are skipped), run by
`fork/tests.sh` after a merge:

```
cargo test -p lightcraft-ui-egui --lib -- map_view
```

A new feature adds its own file here and never edits `fork/tests.sh`, so two branches never touch the same line.
