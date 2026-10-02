#!/bin/sh
# cvid-models.sh: CV ID's models into crates/cvid/models - YuNet and SFace
# from the OpenCV zoo, MiniFASNetV2 (Silent-Face-Anti-Spoofing's, as exported
# to ONNX by yakhyo/face-anti-spoofing); all Apache 2.0.
set -e
cd "$(dirname "$0")/../crates/cvid/models"
Z=https://github.com/opencv/opencv_zoo/raw/main/models
curl -sSL -o face_detection_yunet_2023mar.onnx $Z/face_detection_yunet/face_detection_yunet_2023mar.onnx
curl -sSL -o face_recognition_sface_2021dec.onnx $Z/face_recognition_sface/face_recognition_sface_2021dec.onnx
curl -sSL -o MiniFASNetV2.onnx https://github.com/yakhyo/face-anti-spoofing/releases/download/weights/MiniFASNetV2.onnx
ls -la *.onnx
