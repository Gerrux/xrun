"""Classification eval demo — produces scalar metrics + CM/PR/ROC plots.

Trains logistic regression on synthetic data (4 classes), then for each epoch
logs the usual scalars (loss, acc, f1_macro, etc.) and at the end renders:

  output/confusion_matrix.png
  output/pr_curve.png            (per-class precision-recall vs threshold)
  output/roc_curve.png           (per-class ROC with AUC in legend)
  output/per_class_metrics.json

These are the artefacts the Report tab is meant to surface alongside the
scalar curves. Run, then hit `5` in Run detail to see everything in one page.
"""
from __future__ import annotations

import json
import os
import time

import matplotlib

matplotlib.use("Agg")  # no GUI on headless / CI
import matplotlib.pyplot as plt
import numpy as np
from sklearn.datasets import make_classification
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import (
    auc,
    average_precision_score,
    confusion_matrix,
    f1_score,
    precision_recall_curve,
    precision_score,
    recall_score,
    roc_auc_score,
    roc_curve,
)
from sklearn.model_selection import train_test_split

import xrun_hook

OUT_DIR = "output"
N_CLASSES = 4
N_SAMPLES = 4000
N_EPOCHS = 10


# ── Tokyo-Night style for matplotlib ─────────────────────────────────────────

plt.rcParams.update({
    "figure.facecolor": "#1a1b26",
    "axes.facecolor":   "#1e2030",
    "axes.edgecolor":   "#565f89",
    "axes.labelcolor":  "#c0caf5",
    "xtick.color":      "#c0caf5",
    "ytick.color":      "#c0caf5",
    "text.color":       "#c0caf5",
    "axes.titlecolor":  "#c0caf5",
    "grid.color":       "#2d3149",
    "savefig.facecolor": "#1a1b26",
    "savefig.edgecolor": "#1a1b26",
    "font.size":         10,
})

PALETTE = ["#7aa2f7", "#9ece6a", "#e0af68", "#f7768e",
           "#bb9af7", "#7dcfff", "#ff9e64", "#c0caf5"]


def main() -> None:
    print("classification_eval: starting", flush=True)
    os.makedirs(OUT_DIR, exist_ok=True)

    with xrun_hook.stage("data_load"):
        X, y = make_classification(
            n_samples=N_SAMPLES, n_features=20, n_informative=10,
            n_classes=N_CLASSES, n_clusters_per_class=2, random_state=42,
        )
        X_tr, X_te, y_tr, y_te = train_test_split(
            X, y, test_size=0.25, stratify=y, random_state=42,
        )
        xrun_hook.metric("dataset_size", float(len(X)), step=0)
        xrun_hook.metric("n_classes",    float(N_CLASSES), step=0)
        time.sleep(0.05)

    with xrun_hook.stage("train"):
        clf = LogisticRegression(max_iter=1, warm_start=True)
        for epoch in range(N_EPOCHS):
            clf.set_params(max_iter=clf.max_iter + 30)
            clf.fit(X_tr, y_tr)
            tr_pred = clf.predict(X_tr)
            te_pred = clf.predict(X_te)
            xrun_hook.metrics({
                "train_acc":      float(np.mean(tr_pred == y_tr)),
                "val_acc":        float(np.mean(te_pred == y_te)),
                "train_f1_macro": float(f1_score(y_tr, tr_pred, average="macro")),
                "val_f1_macro":   float(f1_score(y_te, te_pred, average="macro")),
                "lr":             1.0 / (1 + epoch),
            }, step=epoch)
            xrun_hook.epoch(epoch, {
                "val_acc": float(np.mean(te_pred == y_te)),
            })
            time.sleep(0.05)

    with xrun_hook.stage("eval"):
        proba = clf.predict_proba(X_te)
        pred  = clf.predict(X_te)

        # ── Scalar finals ───────────────────────────────────────────────────
        xrun_hook.metric("final_acc",
                         float(np.mean(pred == y_te)), step=0)
        xrun_hook.metric("final_f1_macro",
                         float(f1_score(y_te, pred, average="macro")), step=0)
        xrun_hook.metric("final_f1_micro",
                         float(f1_score(y_te, pred, average="micro")), step=0)
        xrun_hook.metric("final_precision_macro",
                         float(precision_score(y_te, pred, average="macro")), step=0)
        xrun_hook.metric("final_recall_macro",
                         float(recall_score(y_te, pred, average="macro")), step=0)

        # one-vs-rest ROC / PR — averaged AUC + per-class
        roc_auc_macro = float(roc_auc_score(y_te, proba,
                                            multi_class="ovr",
                                            average="macro"))
        xrun_hook.metric("final_roc_auc_macro", roc_auc_macro, step=0)

        per_class = {}
        for cls in range(N_CLASSES):
            y_bin = (y_te == cls).astype(int)
            ap = float(average_precision_score(y_bin, proba[:, cls]))
            ra = float(roc_auc_score(y_bin, proba[:, cls]))
            xrun_hook.metric(f"pr_auc_class{cls}",  ap, step=0)
            xrun_hook.metric(f"roc_auc_class{cls}", ra, step=0)
            per_class[f"class{cls}"] = {"pr_auc": ap, "roc_auc": ra}

        with open(f"{OUT_DIR}/per_class_metrics.json", "w") as f:
            json.dump(per_class, f, indent=2)

        # ── Plots ───────────────────────────────────────────────────────────
        _plot_confusion_matrix(y_te, pred, f"{OUT_DIR}/confusion_matrix.png")
        _plot_pr(y_te, proba, f"{OUT_DIR}/pr_curve.png")
        _plot_roc(y_te, proba, f"{OUT_DIR}/roc_curve.png")
        time.sleep(0.05)

    xrun_hook.done()
    print("classification_eval: done", flush=True)


def _plot_confusion_matrix(y_true, y_pred, path: str) -> None:
    cm = confusion_matrix(y_true, y_pred)
    cm_norm = cm / cm.sum(axis=1, keepdims=True)

    fig, ax = plt.subplots(figsize=(6, 5))
    im = ax.imshow(cm_norm, cmap="viridis", vmin=0, vmax=1)
    fig.colorbar(im, ax=ax, fraction=0.046, pad=0.04)
    ax.set_title("Confusion Matrix (row-normalised)")
    ax.set_xlabel("Predicted")
    ax.set_ylabel("True")
    ax.set_xticks(range(N_CLASSES))
    ax.set_yticks(range(N_CLASSES))
    for i in range(N_CLASSES):
        for j in range(N_CLASSES):
            ax.text(j, i, f"{cm_norm[i, j]:.2f}\n({cm[i, j]})",
                    ha="center", va="center",
                    color="white" if cm_norm[i, j] < 0.6 else "black",
                    fontsize=9)
    fig.tight_layout()
    fig.savefig(path, dpi=120)
    plt.close(fig)


def _plot_pr(y_true, proba, path: str) -> None:
    fig, ax = plt.subplots(figsize=(7, 5))
    for cls in range(N_CLASSES):
        y_bin = (y_true == cls).astype(int)
        prec, rec, _ = precision_recall_curve(y_bin, proba[:, cls])
        ap = average_precision_score(y_bin, proba[:, cls])
        ax.plot(rec, prec,
                color=PALETTE[cls % len(PALETTE)],
                label=f"class{cls}  AP={ap:.3f}",
                linewidth=2)
    ax.set_xlabel("Recall")
    ax.set_ylabel("Precision")
    ax.set_title("Precision–Recall (one-vs-rest)")
    ax.set_xlim(0, 1.02)
    ax.set_ylim(0, 1.02)
    ax.grid(True, linestyle=":")
    ax.legend(loc="lower left", facecolor="#1e2030", edgecolor="#565f89")
    fig.tight_layout()
    fig.savefig(path, dpi=120)
    plt.close(fig)


def _plot_roc(y_true, proba, path: str) -> None:
    fig, ax = plt.subplots(figsize=(7, 5))
    for cls in range(N_CLASSES):
        y_bin = (y_true == cls).astype(int)
        fpr, tpr, _ = roc_curve(y_bin, proba[:, cls])
        a = auc(fpr, tpr)
        ax.plot(fpr, tpr,
                color=PALETTE[cls % len(PALETTE)],
                label=f"class{cls}  AUC={a:.3f}",
                linewidth=2)
    ax.plot([0, 1], [0, 1], color="#565f89", linestyle="--", linewidth=1)
    ax.set_xlabel("False positive rate")
    ax.set_ylabel("True positive rate")
    ax.set_title("ROC (one-vs-rest)")
    ax.set_xlim(0, 1.02)
    ax.set_ylim(0, 1.02)
    ax.grid(True, linestyle=":")
    ax.legend(loc="lower right", facecolor="#1e2030", edgecolor="#565f89")
    fig.tight_layout()
    fig.savefig(path, dpi=120)
    plt.close(fig)


if __name__ == "__main__":
    main()
