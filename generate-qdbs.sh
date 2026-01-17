mkdir qdbs

qunet-cli qdb create -o ./qdbs/4k-l2.qdb --max-size=4096 --zstd-level 2 $1
qunet-cli qdb create -o ./qdbs/8k-l2.qdb --max-size=8192 --zstd-level 2 $1
qunet-cli qdb create -o ./qdbs/16k-l2.qdb --max-size=16384 --zstd-level 2 $1
qunet-cli qdb create -o ./qdbs/32k-l2.qdb --max-size=32768 --zstd-level 2 $1

qunet-cli qdb create -o ./qdbs/4k-l3.qdb --max-size=4096 --zstd-level 3 $1
qunet-cli qdb create -o ./qdbs/8k-l3.qdb --max-size=8192 --zstd-level 3 $1
qunet-cli qdb create -o ./qdbs/16k-l3.qdb --max-size=16384 --zstd-level 3 $1
qunet-cli qdb create -o ./qdbs/32k-l3.qdb --max-size=32768 --zstd-level 3 $1