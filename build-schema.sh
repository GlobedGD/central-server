#!/bin/bash

# Builds the capnproto c++ schemas. Rust schemas are built together with the server.
# Schemas will be generated in `schema/generated/`.

set -e

mkdir -p ./schema/generated

echo "Generating C++ schemas"
capnp compile -oc++:./schema/generated --src-prefix=schema ./schema/main.capnp

echo "Done!"