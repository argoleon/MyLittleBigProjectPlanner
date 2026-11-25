# ClamAV of development and tests: the real engine, without the signature
# databases (about 200 MB, downloaded at start and refreshed hourly) but
# with one test database that detects the EICAR test file. It starts in
# seconds and never reaches the ClamAV mirrors. Production runs the full
# image (deploy/compose.yaml).
FROM clamav/clamav:1.5_base
COPY dev/clamav/test.hdb /var/lib/clamav/test.hdb
# An argument starting with "-" makes the image's /init run clamd at once.
# Without it, /init downloads the official databases first whenever
# main.cvd is missing (CLAMAV_NO_FRESHCLAMD only stops the update daemon):
# the EICAR file then matches the official signature, named
# "Eicar-Test-Signature" instead of "Eicar-Test-Signature.UNOFFICIAL", and
# the tests depend on whether a ClamAV mirror answered.
CMD ["--foreground"]
