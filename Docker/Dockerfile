FROM scratch

EXPOSE 8080/tcp
VOLUME ["/downloads", "/data"]

ARG TARGETPLATFORM

ADD binaries/$TARGETPLATFORM/http-drogue /

WORKDIR /downloads

ENTRYPOINT ["/http-drogue"]
