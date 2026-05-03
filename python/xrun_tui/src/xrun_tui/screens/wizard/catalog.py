"""Static tables consumed by the first-run wizard.

Kept as plain tuples so the data is grep-friendly and easy to diff.
"""
from __future__ import annotations

# (id, label, description, api_key_url, available_now, takes_paste_key)
VENDOR_CARDS = [
    ("vast",   "vast.ai",
        "GPU spot marketplace — primary cloud vendor",
        "https://cloud.vast.ai/account/", True, True),
    ("kaggle", "Kaggle",
        "Free notebooks; live logs via MLflow mirror",
        "https://www.kaggle.com/settings/account", True, True),
    ("ssh",    "SSH machine",
        "Your own server / NAS / VPS over SSH (configure host in Vendors screen)",
        "https://www.openssh.com/manual.html", True, False),
    ("runpod", "RunPod",
        "REST + SSH cloud (v0.7)",
        "https://www.runpod.io/console/user/settings", False, False),
    ("lambda", "Lambda Labs",
        "Stable-priced GPU cloud (v0.7)",
        "https://cloud.lambdalabs.com/api-keys", False, False),
    ("lightning", "Lightning AI",
        "80 free GPU-h/mo (v0.7)",
        "https://lightning.ai/me/settings", False, False),
]

# (id, label, available_now, url)
SINKS = [
    ("mlflow", "MLflow",
        True,  "https://mlflow.org/docs/latest/tracking-server.html"),
    ("wandb",  "WandB",
        False, "https://wandb.ai/authorize"),
    ("comet",  "Comet ML",
        False, "https://www.comet.com/account-settings/apiKeys"),
]

VENDOR_BY_ID = {c[0]: c for c in VENDOR_CARDS}
SINK_BY_ID = {s[0]: s for s in SINKS}

# (field, label, placeholder, required) — SSH form when SSH card is checked.
SSH_FIELDS = [
    ("alias", "Alias",         "myhost (used in manifests as ssh.host_alias)", True),
    ("host",  "Host",          "192.168.1.10 or vps.example.com",              True),
    ("user",  "User",          "root",                                          True),
    ("port",  "Port",          "22 (optional)",                                 False),
    ("key",   "Identity file", "~/.ssh/id_ed25519 (optional)",                  False),
]

# Kaggle auth: token (preferred) OR username+key (legacy). All blank → adapter
# auto-imports ~/.kaggle/kaggle.json. (field, placeholder, password)
KAGGLE_FIELDS = [
    ("token",    "JWT access token (Account → Tokens → Create new) — preferred", True),
    ("username", "or legacy username (from kaggle.json)",                         False),
    ("key",      "or legacy API key (paired with username)",                      True),
]

# MLflow form. URL is required; auth is optional (token, OR user+password,
# OR none for an anonymous server). (field, placeholder, password)
MLFLOW_FIELDS = [
    ("url",      "Tracking URL — http://localhost:5000 or https://mlflow.company.com (REQUIRED)", False),
    ("token",    "Bearer token (preferred) — or leave blank for Basic / anonymous",                True),
    ("username", "or Basic-auth username",                                                          False),
    ("password", "or Basic-auth password (paired with username)",                                   True),
]

_FOCUS_URL_PREFIXES = {
    "wiz-vendor-cb-":    lambda vid: VENDOR_BY_ID.get(vid, (None,) * 4)[3],
    "wiz-vendor-input-": lambda vid: VENDOR_BY_ID.get(vid, (None,) * 4)[3],
    "wiz-sink-cb-":      lambda sid: SINK_BY_ID.get(sid, (None,) * 4)[3],
    "wiz-ssh-":          lambda _f: VENDOR_BY_ID["ssh"][3],
    "wiz-kaggle-":       lambda _f: VENDOR_BY_ID["kaggle"][3],
    "wiz-mlflow-":       lambda _f: SINK_BY_ID["mlflow"][3],
}


def focus_url(widget_id: str | None) -> str | None:
    """Map a widget id to the relevant API-key page (used by `o` binding)."""
    if not widget_id:
        return None
    for prefix, lookup in _FOCUS_URL_PREFIXES.items():
        if widget_id.startswith(prefix):
            return lookup(widget_id[len(prefix):])
    return None
