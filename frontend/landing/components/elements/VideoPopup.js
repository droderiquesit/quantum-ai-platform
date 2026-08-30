'use client'
import { useRef } from 'react'

// A video lightbox with no video is a lie on a landing page, so this renders
// nothing unless a caller supplies a real videoId. The template shipped a
// stock YouTube demo here (and a react-modal-video dependency pinned to
// React 18); both are gone — when the desk records an actual product video,
// pass its id and the native <dialog> below plays it with no dependency.
export default function VideoPopup({ style, text, videoId }) {
    const dialogRef = useRef(null)
    if (!videoId) return null

    const open = () => dialogRef.current?.showModal()
    const close = () => dialogRef.current?.close()

    const trigger = (
        <a onClick={open} className="overlay-link lightbox-image video-fancybox ripple">
            <span className={style ? 'icon-10' : 'icon-11'} />
        </a>
    )

    return (
        <>
            {!style && trigger}
            {style >= 1 && (
                <div className="video-btn">
                    {trigger}
                    {style === 2 && <h6>{text || 'Latest Program Video'}</h6>}
                </div>
            )}
            <dialog ref={dialogRef} onClick={close} style={{ padding: 0, border: 0, background: 'transparent', maxWidth: '90vw' }}>
                <iframe
                    title="video"
                    width="960"
                    height="540"
                    style={{ maxWidth: '90vw', maxHeight: '80vh', display: 'block', border: 0 }}
                    src={`https://www.youtube-nocookie.com/embed/${videoId}?autoplay=1`}
                    allow="autoplay; encrypted-media"
                    allowFullScreen
                />
            </dialog>
        </>
    )
}
