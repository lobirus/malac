# ponytail

These skills are [ponytail](https://ponytail.dev) 5.3.2 by Dietrich Gebert
(<https://github.com/DietrichGebert/ponytail>, commit `09c434a`, 2026-10-18),
copied unchanged under the MIT license in [LICENSE](LICENSE).

malac ships them as its default best-practices plugin, `ponytail@malac`. Only
the skills are included: ponytail's hooks, status line and task board are
written for other agents and run Node.js.

- Turn it off: `malac plugin disable ponytail@malac`, or `[plugins] defaults = false`
  in `~/.malac/config.toml`.
- Use upstream instead: `malac plugin marketplace add DietrichGebert/ponytail`, then
  `malac plugin install ponytail@ponytail`. An installed ponytail replaces this copy.
