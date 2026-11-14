"""Guard ESP-Hosted 1.4.7 streaming SDIO RX against buffer exhaustion."""
import hashlib
from pathlib import Path
import sys

UPSTREAM_SHA256 = '9868f97b7c68e5d8f1a3ac904dc6922b8ba0cc160c28c6a1402cd5c4147a652a'
OLD = '''\t\t/* Allocate rx buffer */
\t\tpkt_rxbuff = sdio_buffer_alloc(MEMSET_REQUIRED);
\t\tassert(pkt_rxbuff);

\t\tpacket_size = len + offset;
\t\tif (packet_size > buf_len) {
\t\t\tESP_LOGE(TAG, "packet size too big for remaining stream data");
\t\t\treturn ESP_FAIL;
\t\t}'''
NEW = '''\t\tpacket_size = len + offset;
\t\tif (packet_size > buf_len) {
\t\t\tESP_LOGE(TAG, "packet size too big for remaining stream data");
\t\t\treturn ESP_FAIL;
\t\t}
\t\t/* Dropping the remaining stream is safer than crashing on OOM. */
\t\tpkt_rxbuff = sdio_buffer_alloc(MEMSET_REQUIRED);
\t\tif (!pkt_rxbuff) {
\t\t\tESP_LOGE(TAG, "SDIO RX buffer exhausted; dropping stream");
\t\t\treturn ESP_ERR_NO_MEM;
\t\t}'''


def patch(source: str) -> str:
    if hashlib.sha256(source.encode('utf-8')).hexdigest() != UPSTREAM_SHA256 or source.count(OLD) != 1:
        raise ValueError('unexpected ESP-Hosted SDIO source; review before rebuilding')
    return source.replace(OLD, NEW, 1)


if __name__ == '__main__':
    source, output = map(Path, sys.argv[1:])
    result = patch(source.read_text(encoding='utf-8'))
    if not output.exists() or output.read_text(encoding='utf-8') != result:
        output.write_text(result, encoding='utf-8')
