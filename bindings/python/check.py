from octri_converter import _json as exact_json
import sys
from octri_converter import Converter

converter = Converter(sys.argv[1])
envelope = exact_json.loads(sys.stdin.read())
action = envelope.get("action")
import os
if os.environ.get("OCTRI_BINDING_RAW") == "1" or envelope.get("abiVersion") != 1 or action not in ("convert", "review", "bundle", "compare", "export", "formats", "detect", "batch"):
    result = converter.call(envelope)
elif action in ("convert", "review", "bundle"):
    result = {"Ok": getattr(converter, action)(envelope["request"])}
elif action == "detect":
    result = {"Ok": converter.detect(envelope["request"])}
elif action == "batch":
    result = {"Ok": converter.batch(envelope["requests"])}
elif action == "compare":
    result = {"Ok": converter.compare(envelope["before"], envelope["after"])}
elif action == "export":
    result = {"Ok": converter.export(envelope["report"], envelope["format"])}
else:
    result = {"Ok": converter.formats()}
print(exact_json.dumps(result))
