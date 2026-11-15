# ClamAV of development and tests: the real engine, without the signature
# databases (about 200 MB, downloaded at start and refreshed hourly) but
# with one test database that detects the EICAR test file. It starts in
# seconds and never reaches the ClamAV mirrors. Production runs the full
# image (deploy/compose.yaml).
FROM clamav/clamav:1.5_base
COPY clamav/test.hdb /var/lib/clamav/test.hdb
