# Shared fixture adapter

`scripts/suite-qualification.mjs` consumes the existing fractional delivery,
typed timeline and movable archive assertions. The shared contract and candidate
index live in Omadesign's `qualification/suite/index.json`; the shared runner owns
integrity, exact source/binary/tool bindings, the global build lock, cancellation
and immutable receipt storage. No codec, worker, format or UI API changes here.

The tiny media generator produces 32x18 numbered RGBA/alpha frames and unequal
channel content on a 30000/1001 clock. Lossless MOV checks every pixel/sample;
H.264 and ProRes retain their existing explicit RMS/priming bounds. Writer and
reader share FFmpeg codecs, so this proves the declared mathematical sample
contract, not independent codec conformance. The timeline fixture retains typed
edit/undo/save/recovery and stale action refusal. The archive fixture moves,
reopens, saves and recovers without the original media, then refuses a mutated
member without publishing a replacement project.

`node scripts/suite-qualification.mjs --plan` lists the exact existing assertions.
`node --test scripts/suite-qualification.test.mjs` checks that a command returning
zero without executing a test cannot pass, and that a missing executable fails.
Neither command builds or opens the product.

Runtime requests are private JSON `{fixture:{id},input,output,binary,driver:
"libtest"}` passed only by the shared serialized runner. Each exact test must
report one pass and zero ignored tests. A build record must name the current
clean source/tree and actual integration-test executable for that fixture.
Preserved assertion logs and JSON are output artifacts. Missing input/reference,
zero-test output and obsolete source remain unqualified. No installed app,
native window, display timing, audio device, physical or human proof is inferred.

New editorial/caption/scopes/multicam and VIDEO-08 codec/HDR producer APIs remain
with the media owner. Consume their pinned manifests/outputs after qualification;
this adapter supplies no alternate pipeline and does not promote old receipts.
