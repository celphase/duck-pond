# Attribution

duck-pond contains no recorded audio. Every sound is synthesized, but the
calls, the ducks' behavior and the way sound travels across the pond were
tuned against the studies below.

## Duck calls

- **Gottlieb, G. (1982).** Development of species identification in
  ducklings: IX. The necessity of experiencing normal variations in embryonic
  auditory stimulation. *Developmental Psychobiology* 15(6): 507–517.
  [doi:10.1002/dev.420150603](https://doi.org/10.1002/dev.420150603)
  Used for the rate of the hen's brood call, about 3.7 notes a second.
- **Gaioni, S. J. & Evans, C. S.** The role of frequency modulation in
  controlling the response of mallard ducklings (*Anas platyrhynchos*) to
  conspecific distress calls.
  [Macquarie University](https://researchers.mq.edu.au/en/publications/the-role-of-frequency-modulation-in-controlling-the-response-of-m/)
  Used for duckling distress peeps: a 267 ms note period and a falling sweep
  at the end of each note.
- ***The influence of behavioral context on the vocalizations of Mallard
  ducklings (Anas platyrhynchos)*** (1998). PhD dissertation, University of
  Connecticut.
  [AAI9909786](https://digitalcommons.lib.uconn.edu/dissertations/AAI9909786)
  Used to separate the two duckling calls: loud distress peeps only when a
  duckling is cut off from its brood, and shorter, quieter contact notes
  otherwise.

## Behavior and movement

- **Pietz, P. J. & Buhl, D. A. (1999).** Behaviour patterns of Mallard *Anas
  platyrhynchos* pairs and broods in Minnesota and North Dakota. *Wildfowl* 50:
  101–122.
  [USGS](https://www.usgs.gov/publications/behaviour-patterns-mallard-anas-platyrhynchos-pairs-and-broods-minnesota-and-north)
  Used for how often mates, and hens and their broods, do the same thing
  (about two thirds of the time), and for what each tends to do when they
  differ.
- **Asplund, C. (1981).** Time budgets of breeding Mallard in northern Sweden.
  *Wildfowl* 32: 55–64.
  [Wildfowl](https://wildfowl.wwt.org.uk/index.php/wildfowl/article/view/628)
  Its feeding figures for breeding hens are one of the rough guides behind
  the time budget (about 40 % feeding, 30 % resting, 15 % preening).
- **Turnbull, R. E. & Baldassarre, G. A. (1987).** Activity budgets of
  Mallards and American Wigeon wintering in east-central Alabama. *Wilson
  Bulletin* 99(3): 457–464.
  [SORA](https://sora.unm.edu/sites/default/files/journals/wilson/v099n03/p0457-p0464.pdf)
  Its resting figures are the other rough guide behind the time budget.
- **Aigeldinger, T. L. & Fish, F. E. (1995).** Hydroplaning by ducklings:
  overcoming limitations to swimming at the water surface. *Journal of
  Experimental Biology* 198: 1567–1574.
  [PDF](https://health-sciences.wcupa.edu/sciences-mathematics/biology/fFish/documents/1995JEBHydroplane.pdf)
  Used for duckling speeds: paddling below hull speed (0.43 m/s), bursting
  faster only to catch up.
- **Prange, H. D. & Schmidt-Nielsen, K. (1970).** The metabolic cost of
  swimming in ducks. *Journal of Experimental Biology* 53: 763–777.
  [doi:10.1242/jeb.53.3.763](https://doi.org/10.1242/jeb.53.3.763)
  Used for adult swimming speeds, which stay below hull speed.

## Sound propagation and spatial audio

- **ISO 9613-1:1993.** Acoustics — Attenuation of sound during propagation
  outdoors — Part 1: Calculation of the absorption of sound by the atmosphere.
  Used for air absorption with distance (20 °C, 70 % humidity), which sets
  the distance lowpass and is checked by a test.
- **Brown, C. P. & Duda, R. O. (1998).** A structural model for binaural sound
  synthesis. *IEEE Transactions on Speech and Audio Processing* 6(5): 476–488.
  Used for the spherical head model in `--headphones` mode.
- **Woodworth, R. S. (1938).** *Experimental Psychology*. New York: Holt.
  Used for the interaural time difference of a spherical head.

Also consulted when evaluating the distance model, though nothing in the code
comes from them: Richards, D. G. (1981), [Environmental acoustics and censuses
of singing birds](https://sora.unm.edu/sites/default/files/SAB_006_1981_P297-300%20Part%206%20Environmental%20Acoustics%20and%20Censuses%20of%20Singing%20Birds%20Douglas%20G.%20Richards.pdf),
*Studies in Avian Biology* 6: 297–300; and Mercado, E. et al. (2013),
Reverberlocation in chickadees?, *Proceedings of Meetings on Acoustics* 19,
[doi:10.1121/1.4799208](https://doi.org/10.1121/1.4799208). They describe
how reverberation changes a call with distance, and the second uses the
balance of direct sound to reverb as a distance cue. That cue is why a
reflection off the water was tried and then left out.

## Signal processing

- **Butterworth, S. (1930).** On the theory of filter amplifiers. *Wireless
  Engineer* 7: 536–541. Highpass and lowpass filters on each call.
- **Catmull, E. & Rom, R. (1974).** A class of local interpolating splines.
  In *Computer Aided Geometric Design*, Academic Press. Fractional delays
  for each ear.
- **Freeverb**, by Jezar at Dreampoint (public domain). Design of the pond's
  reverb.

## Reference recordings and tools

These were used while developing the Python prototype, to measure and check
the synthesized calls. They are not part of this
repository.

- **Douglas Von Gausig, Naturesongs.com.** Mallard recordings
  [mallard1](https://animaldiversity.org/collections/contributors/naturesongs/mallard1/)
  (two hens quacking) and
  [mall2](https://www.animaldiversity.org/collections/contributors/naturesongs/mall2/)
  (a drake dabbling), via Animal Diversity Web, licensed
  [CC BY-NC-SA 3.0](https://creativecommons.org/licenses/by-nc-sa/3.0/).
  The quack's pitch, formants, length and timing were measured from these.
- **Kong, Q. et al. (2020).** PANNs: Large-scale pretrained audio neural
  networks for audio pattern recognition. *IEEE/ACM Transactions on Audio,
  Speech, and Language Processing* 28: 2880–2894.
  [Weights](https://zenodo.org/records/3987831)
  The CNN14 model was used to check that the synthesized calls are recognized
  as "Duck" and "Quack".
- **Gemmeke, J. F. et al. (2017).** Audio Set: An ontology and human-labeled
  dataset for audio events. *ICASSP 2017*. The labels the PANNs model was
  trained on.

Some of these studies were read only as abstracts or summaries, because the
full text was paywalled or could not be fetched.
