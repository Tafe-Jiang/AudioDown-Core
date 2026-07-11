FROM node:22-bookworm-slim@sha256:53ada149d435c38b14476cb57e4a7da73c15595aba79bd6971b547ceb6d018bf

ARG BASE_IMAGE_DIGEST
ARG SDK_HASH
ARG POLICY_VERSION=1.0

RUN groupadd --gid 10001 audiodown-builder \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin audiodown-builder \
    && mkdir -p /opt/audiodown /workspace \
    && chown 10001:10001 /workspace

COPY docker/plugin-runtime/node22-build-runner.js /opt/audiodown/node22-build-runner.js

LABEL io.audiodown.trusted-image="true" \
    io.audiodown.trusted-image-kind="node22-builder" \
    io.audiodown.base-image-digest="${BASE_IMAGE_DIGEST}" \
    io.audiodown.sdk-hash="${SDK_HASH}" \
    io.audiodown.build-policy-version="${POLICY_VERSION}"

WORKDIR /workspace
USER 10001:10001

ENTRYPOINT ["node", "/opt/audiodown/node22-build-runner.js"]
