# The specification, as fm tests it

These pages are a copy of `site/content/` from
[agent-automations](https://github.com/lucas-stanford/agent-automations) at `6385d48`. fm is the
reference implementation: `crates/fm-spec/tests/conformance.rs` parses every example marked
`automation` or `frontmatter` and checks it is accepted or refused exactly as the page says.

When the specification changes, copy the pages again and run the tests:

```sh
cp ../agent-automations/site/content/*.md tests/spec/
cargo test -p fm-spec --test conformance
```
