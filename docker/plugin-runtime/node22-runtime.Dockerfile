FROM node:22-bookworm-slim@sha256:53ada149d435c38b14476cb57e4a7da73c15595aba79bd6971b547ceb6d018bf

ARG BASE_IMAGE_DIGEST
ARG SDK_HASH
ARG POLICY_VERSION=1.0

RUN groupadd --gid 10002 audiodown-plugin \
    && useradd --uid 10002 --gid 10002 --no-create-home --shell /usr/sbin/nologin audiodown-plugin \
    && mkdir -p /plugin /sdk /run/audiodown /tmp \
    && chown -R 10002:10002 /plugin /sdk /run/audiodown /tmp

COPY --chown=10002:10002 plugin-sdk/node/ /sdk/
COPY --chmod=0555 docker/plugin-runtime/plugin-token-bootstrap.sh /usr/local/bin/audiodown-plugin-bootstrap

LABEL io.audiodown.trusted-image="true" \
    io.audiodown.trusted-image-kind="node22-runtime" \
    io.audiodown.base-image-digest="${BASE_IMAGE_DIGEST}" \
    io.audiodown.sdk-hash="${SDK_HASH}" \
    io.audiodown.build-policy-version="${POLICY_VERSION}"

ENV AUDIODOWN_NODE_SDK_PATH=/sdk/src/index.js \
    NODE_ENV=production

WORKDIR /plugin
USER 10002:10002
