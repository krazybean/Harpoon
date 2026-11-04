FROM alpine:3.22
# Synthetic, deterministic build load: two CPU-bound workers, then a held anonymous-memory
# allocation. Sizes are build args so the harness can scale them.
ARG CPU_MB=2000
ARG MEM_MB=768
RUN set -e; for i in 1 2; do (head -c $((CPU_MB*1048576)) /dev/zero | sha256sum >/dev/null) & done; wait
RUN awk -v mb="$MEM_MB" 'BEGIN { s = sprintf("%1048576s", "x"); for (i = 0; i < mb; i++) a[i] = s i; system("sleep 3") }'
RUN echo built > /built
CMD ["cat", "/built"]
