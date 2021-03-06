# Copyright (c) 2017, 2020 ADLINK Technology Inc.
#
# This program and the accompanying materials are made available under the
# terms of the Eclipse Public License 2.0 which is available at
# http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
# which is available at https://www.apache.org/licenses/LICENSE-2.0.
#
# SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
#
# Contributors:
#   ADLINK zenoh team, <zenoh@adlink-labs.tech>

###
### Dockerfile running the Eclipse zenoh router (zenohd)
###
# To build this Docker image:
#   - Build zenoh using the adlinktech/zenoh-dev-x86_64-unknown-linux-musl docker image:
#       docker run --init --rm -v $(pwd):/workdir -w /workdir adlinktech/zenoh-dev-x86_64-unknown-linux-musl cargo build --release --bins --lib --examples
#   - Then build this Docker image:
#       docker build -t eclipse/zenoh .
#
# To run this Docker image:
#    docker run --init -p 7447:7447/tcp -p 7447:7447/udp -p 8000:8000/tcp eclipse/zenoh

FROM rust:latest as builder

WORKDIR /root/zenoh

COPY . .

RUN cargo build --release

RUN cargo build --release --example zn_pub --example zn_sub

FROM rust:slim

ARG REPO

LABEL org.opencontainers.image.source ${REPO}

COPY --from=builder /root/zenoh/target/release/zenohd /
COPY --from=builder /root/zenoh/target/release/*.so /
COPY --from=builder /root/zenoh/target/release/examples/zn_pub /
COPY --from=builder /root/zenoh/target/release/examples/zn_sub /

RUN echo '#!/bin/ash' > /entrypoint.sh
RUN echo 'echo " * Starting: /zenohd --mem-storage=/rsu/** $*"' >> /entrypoint.sh
RUN echo 'exec /zenohd --mem-storage=/rsu/** $*' >> /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 7447/udp
EXPOSE 7447/tcp
EXPOSE 8000/tcp

ENV RUST_LOG info

ENTRYPOINT ["/entrypoint.sh"]
