import sys
import glob
import runpy
import os

# Inject xrun_hook wheel into sys.path if present
whl = glob.glob('xrun_hook-*-py3-none-any.whl')
if whl:
    sys.path.insert(0, whl[0])

# Set up run directory
os.environ.setdefault('XRUN_RUN_DIR', '/kaggle/working/run')
os.makedirs(os.environ['XRUN_RUN_DIR'], exist_ok=True)

# Import xrun_hook to install excepthook and set up logging
try:
    import xrun_hook  # noqa: F401
except ImportError:
    pass  # gracefully degrade if wheel not available

# Run the target script
target = os.environ.get('XRUN_TARGET_SCRIPT')
if target:
    runpy.run_path(target, run_name='__main__')
else:
    raise RuntimeError(
        'XRUN_TARGET_SCRIPT environment variable not set. '
        'This wrapper requires XRUN_TARGET_SCRIPT to point to the user script.'
    )
