import './lib/excalidrawSystemFontsBootstrap';

if (import.meta.env.VITE_MMD_PACKAGED_LIFECYCLE_E2E === '1') {
  void import('./lib/packagedLifecycleE2e').then(({ startPackagedLifecycleE2e }) => (
    startPackagedLifecycleE2e()
  ));
} else {
  void import('./main');
}
