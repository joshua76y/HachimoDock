"""Generate a guarded copy of ESP-Hosted 1.4.7's transport source.

The managed component remains untouched.  A failed DMA-buffer allocation must
be returned to esp-netif instead of asserting in the TCP/IP task.  Refuse other
upstream revisions until their transmit paths have been reviewed.
"""
import hashlib
from pathlib import Path
import sys

UPSTREAM_SHA256 = 'a864382a1b5c2a585bc2a2dccca61dcd25b0533367ccd62e6e895d202ba9a413'
OLD = '\tassert(copy_buff);'
NEW = '''\tif (!copy_buff) {
\t\terrno = ENOBUFS;
#if defined(ESP_ERR_ESP_NETIF_TX_FAILED)
\t\treturn ESP_ERR_ESP_NETIF_TX_FAILED;
#else
\t\treturn ESP_ERR_ESP_NETIF_NO_MEM;
#endif
\t}'''


def patch(source: str) -> str:
    if hashlib.sha256(source.encode('utf-8')).hexdigest() != UPSTREAM_SHA256 or source.count(OLD) != 2:
        raise ValueError('unexpected ESP-Hosted transport source; review before rebuilding')
    return source.replace(OLD, NEW)


if __name__ == '__main__':
    source, output = map(Path, sys.argv[1:])
    result = patch(source.read_text(encoding='utf-8'))
    if not output.exists() or output.read_text(encoding='utf-8') != result:
        output.write_text(result, encoding='utf-8')
