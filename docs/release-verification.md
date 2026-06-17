# Release verification

Every `vettd` GitHub Release ships three verification artifacts alongside the
platform binaries:

| File | Contents |
|---|---|
| `checksums.txt` | SHA-256 hash for each release asset (standard `sha256sum` format) |
| `checksums.txt.sig` | AWS KMS ECDSA-SHA256 signature over `checksums.txt` (JSON envelope) |
| *(embedded in binary)* | The ECDSA public key baked in at compile time via `VETTD_UPDATE_PUBLIC_KEY_DER_B64` |

The KMS key used to sign `checksums.txt` is the same key used by `vettd update`
to verify the self-update manifest, so both flows share a single root of trust.

---

## Quick checksum verification

Download the binary and `checksums.txt` for your platform, then:

```bash
# Linux
sha256sum --check --ignore-missing checksums.txt

# macOS
shasum -a 256 --check --ignore-missing checksums.txt
```

Expected output (example):

```
vettd-linux-amd64.tar.gz: OK
```

---

## Advanced: verify the KMS signature

`checksums.txt.sig` is a JSON envelope containing the Base64-encoded DER
signature produced by AWS KMS. You can verify it offline using the public key
embedded in any official `vettd` binary.

### 1. Extract the public key from the binary

The ECDSA public key is baked into every official release binary at compile
time.  Use `strings` to extract it:

```bash
strings ./vettd | grep -E '^[A-Za-z0-9+/]{60,}={0,2}$' | head -5
```

The key is the long Base64-encoded DER blob emitted by the `strings` output.
You can also obtain it from the release notes or the `checksums.txt` companion
page published with each GitHub Release.

### 2. Decode and prepare the key

```bash
# Replace <BASE64_KEY> with the value of VETTD_UPDATE_PUBLIC_KEY_DER_B64
echo "<BASE64_KEY>" | base64 -d > pubkey.der
openssl ec -inform DER -pubin -in pubkey.der -pubout -out pubkey.pem
```

### 3. Decode the signature

```bash
# Extract the Base64 signature from checksums.txt.sig
SIG_B64=$(python3 -c "import json,sys; print(json.load(open('checksums.txt.sig'))['signature'])")
echo "$SIG_B64" | base64 -d > checksums.sig.bin
```

### 4. Verify

```bash
openssl dgst -sha256 -verify pubkey.pem -signature checksums.sig.bin checksums.txt
```

Expected output:

```
Verified OK
```

---

## What the signature covers

`checksums.txt.sig` signs the exact bytes of `checksums.txt` — one line per
asset in the format `<sha256hex>  <filename>` (two spaces, sorted by filename).
The same hash values appear in `latest.json` (the self-update manifest), so the
two verification paths are consistent.

---

## Relationship to `vettd update`

`vettd update` fetches `latest.json` from S3 and verifies its detached KMS
signature before trusting any artifact URL or hash. The manual verification
flow described here uses the same KMS key and the same signature algorithm
(ECDSA-SHA256), so both flows reduce to the same root of trust.

Source builds (without `VETTD_UPDATE_PUBLIC_KEY_DER_B64` set at compile time)
will not embed a public key. In that case only the SHA-256 checksum step is
available for manual verification; the signature step requires an official
release binary.
