#!/bin/sh
# cvid-models.sh: CV ID's models from the OpenCV zoo (Apache 2.0) into
# crates/cvid/models.
set -e
cd "$(dirname "$0")/../crates/cvid/models"
Z=https://github.com/opencv/opencv_zoo/raw/main/models
curl -sSL -o face_detection_yunet_2023mar.onnx $Z/face_detection_yunet/face_detection_yunet_2023mar.onnx
curl -sSL -o face_recognition_sface_2021dec.onnx $Z/face_recognition_sface/face_recognition_sface_2021dec.onnx
ls -la *.onnx
