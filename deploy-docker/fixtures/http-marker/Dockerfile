# Disposable test application for Harpoon's first live deployment (B7).
# Local base image only (never pulled), no network access during the build, no volumes, no
# extra packages. Serves /health with a per-deployment marker taken from the environment.
FROM alpine:3.22
COPY srv.sh /srv.sh
RUN chmod 0555 /srv.sh
EXPOSE 8080
CMD ["nc", "-lk", "-p", "8080", "-e", "/srv.sh"]
