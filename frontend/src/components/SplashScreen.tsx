import { useRef, useCallback, useState, useEffect } from 'react';

interface SplashScreenProps {
  onComplete: () => void;
}

export default function SplashScreen({ onComplete }: SplashScreenProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [videoFailed, setVideoFailed] = useState(false);

  const handleEnd = useCallback(() => {
    onComplete();
  }, [onComplete]);

  const handleClick = useCallback(() => {
    onComplete();
  }, [onComplete]);

  const handleError = useCallback(() => {
    setVideoFailed(true);
  }, []);

  useEffect(() => {
    if (videoFailed) {
      const timer = setTimeout(onComplete, 2500);
      return () => clearTimeout(timer);
    }
  }, [videoFailed, onComplete]);

  return (
    <div
      className="splash-screen"
      onClick={handleClick}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => e.key === 'Enter' && onComplete()}
      aria-label="Click to skip"
    >
      {!videoFailed ? (
        <video
          ref={videoRef}
          src="/video/startup.mp4"
          autoPlay
          muted
          playsInline
          onEnded={handleEnd}
          onError={handleError}
          className="splash-video"
        />
      ) : (
        <div className="splash-fallback">
          <h1 className="splash-fallback-title">ORION</h1>
          <p className="splash-fallback-sub">initializing...</p>
        </div>
      )}
      <button
        type="button"
        className="splash-skip"
        onClick={(e) => {
          e.stopPropagation();
          onComplete();
        }}
      >
        [skip]
      </button>
    </div>
  );
}
