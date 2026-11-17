# uqr (vendored)

`uqr.js` is the QR code encoder from [unjs/uqr](https://github.com/unjs/uqr), a port of Project
Nayuki's QR Code generator. It has no dependencies and makes no requests. The Connect phone
dialog (`src/connect-phone.ts`, through `src/quick-connect-qr.ts`) loads it lazily to draw the
Quick connect pairing code on this computer; nothing about the link leaves the app.

- Source: `uqr@0.1.2` on npm, file `dist/index.mjs` (copied as `uqr.js`, unmodified) and
  `dist/index.d.ts` (as `uqr.d.ts`, unmodified)
- Tarball integrity: `sha512-MJu7ypHq6QasgF5YRTjqscSzQp/W11zoUk6kvmlH+fmWEs63Y0Eib13hYFwAzagRJcVY8WVnlV+eBDUGMJ5IbA==`
- SHA-256 of `uqr.js`: `4b96346fc4d721f23e3cab19c06e201577d26f03c5d3dc6decc1468cd0a0d6c0`
- License: MIT, see `LICENSE` (Copyright (c) Project Nayuki; Copyright (c) 2023 Anthony Fu)

It is vendored rather than installed so the encoder is pinned byte for byte and the build
needs no new package. To update: `npm pack uqr@<version> --ignore-scripts`, copy the two
files, and update the version, integrity and digest above and in `THIRD_PARTY_NOTICES.md`.
