# notes-api

A small HTTP service for notes, in Rust: create one, read one, list them,
behind an access token.

It is orchestrated by [nunki](https://github.com/ArnaudSene/nunki): a coder
writes the API and its tests, an integrator wires it to a real PostgreSQL and
writes the system tests, and a security agent attacks the running service.

The battery every commit owes — formatting, clippy, tests, licences and
advisories — runs in CI and in the agents' container.
