(function (global) {
  "use strict";

  const MAX_SOURCE_BYTES = 12 * 1024 * 1024;
  const MAX_DATA_URL_CHARS = 2200000;
  const MAX_WIDTH = 1920;
  const MAX_HEIGHT = 1080;
  const SUPPORTED_TYPES = new Set(["image/jpeg", "image/png", "image/webp"]);

  function fitDimensions(width, height, maxWidth = MAX_WIDTH, maxHeight = MAX_HEIGHT) {
    if (!(width > 0) || !(height > 0)) return { width: 1, height: 1 };
    const scale = Math.min(1, maxWidth / width, maxHeight / height);
    return {
      width: Math.max(1, Math.round(width * scale)),
      height: Math.max(1, Math.round(height * scale)),
    };
  }

  function validateFile(file) {
    if (!file || !SUPPORTED_TYPES.has(file.type)) {
      return "Choose a JPEG, PNG, or WebP image.";
    }
    if (file.size > MAX_SOURCE_BYTES) {
      return "Choose an image smaller than 12 MB.";
    }
    return "";
  }

  function compressWallpaper(file) {
    const error = validateFile(file);
    if (error) return Promise.reject(new Error(error));

    return new Promise((resolve, reject) => {
      const objectUrl = URL.createObjectURL(file);
      const image = new Image();
      image.onload = () => {
        try {
          const size = fitDimensions(image.naturalWidth, image.naturalHeight);
          const canvas = document.createElement("canvas");
          canvas.width = size.width;
          canvas.height = size.height;
          const context = canvas.getContext("2d", { alpha: false });
          context.fillStyle = "#000000";
          context.fillRect(0, 0, size.width, size.height);
          context.drawImage(image, 0, 0, size.width, size.height);
          let dataUrl = canvas.toDataURL("image/webp", 0.82);
          if (dataUrl.length > MAX_DATA_URL_CHARS) {
            dataUrl = canvas.toDataURL("image/webp", 0.64);
          }
          if (dataUrl.length > MAX_DATA_URL_CHARS) {
            reject(new Error("This image is still too large after optimization."));
          } else {
            resolve(dataUrl);
          }
        } catch (reason) {
          reject(reason);
        } finally {
          URL.revokeObjectURL(objectUrl);
        }
      };
      image.onerror = () => {
        URL.revokeObjectURL(objectUrl);
        reject(new Error("The selected image could not be decoded."));
      };
      image.src = objectUrl;
    });
  }

  global.AppearanceFmt = {
    MAX_DATA_URL_CHARS,
    fitDimensions,
    validateFile,
    compressWallpaper,
  };
})(typeof window !== "undefined" ? window : globalThis);
